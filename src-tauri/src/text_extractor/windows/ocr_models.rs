use super::super::local_ocr::ModelStatus;
use super::ocr::Ocr;
use image::RgbaImage;
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

const MAX_MODEL_BYTES: u64 = 32 * 1024 * 1024;
const CACHE_VERSION: &str = "ppocr-v5-cyrillic-rapidocr-3.9.2";
// URLs and digests are pinned to RapidOCR's model registry. Never load unverified weights.
const MODELS: [Model; 2] = [
    Model {
        name: "detector.onnx",
        sources: [
            ModelSource {
                name: "ModelScope",
                url: "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv5/det/ch_PP-OCRv5_det_mobile.onnx",
            },
            ModelSource {
                name: "Hugging Face",
                url: "https://huggingface.co/DjB314/RapidOCR/resolve/04e88d0483f0bfcdd0f29429594244c10bfc2867/v3.9.2/onnx/PP-OCRv5/det/ch_PP-OCRv5_det_mobile.onnx",
            },
        ],
        sha256: "4d97c44a20d30a81aad087d6a396b08f786c4635742afc391f6621f5c6ae78ae",
    },
    Model {
        name: "recognizer.onnx",
        sources: [
            ModelSource {
                name: "ModelScope",
                url: "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv5/rec/cyrillic_PP-OCRv5_rec_mobile.onnx",
            },
            ModelSource {
                name: "Hugging Face",
                url: "https://huggingface.co/DjB314/RapidOCR/resolve/04e88d0483f0bfcdd0f29429594244c10bfc2867/v3.9.2/onnx/PP-OCRv5/rec/cyrillic_PP-OCRv5_rec_mobile.onnx",
            },
        ],
        sha256: "90f761b4bfcce0c8c561c0cb5c887b0971d3ec01c32164bdf7374a35b0982711",
    },
];
struct ModelSource {
    name: &'static str,
    url: &'static str,
}
struct Model {
    name: &'static str,
    sources: [ModelSource; 2],
    sha256: &'static str,
}

enum DownloadError {
    Remote(String),
    Local(&'static str),
}

fn connection_error(error: reqwest::Error) -> DownloadError {
    // Signed CDN URLs contain temporary credentials. Keep them out of logs/UI.
    let reason = if error.is_timeout() {
        "connection timed out"
    } else {
        "connection failed"
    };
    eprintln!("Offline model download: {:#}", error.without_url());
    DownloadError::Remote(reason.into())
}

struct Setup {
    enabled: bool,
    cancel: CancellationToken,
    status: ModelStatus,
    ready: bool,
}

/// Download and session initialization never run on the shortcut/selection thread.
pub struct LocalOcr {
    directory: PathBuf,
    setup: Mutex<Setup>,
    preparing: tokio::sync::Mutex<()>,
    engine: Mutex<Option<Ocr>>,
}
impl LocalOcr {
    pub fn new(data_directory: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            directory: data_directory.join("ocr").join(CACHE_VERSION),
            setup: Mutex::new(Setup {
                enabled: false,
                cancel: CancellationToken::new(),
                status: ModelStatus::new("idle"),
                ready: false,
            }),
            preparing: tokio::sync::Mutex::new(()),
            engine: Mutex::new(None),
        })
    }

    pub fn status(&self) -> ModelStatus {
        self.setup.lock().unwrap().status.clone()
    }

    pub fn set_enabled(self: &Arc<Self>, enabled: bool) {
        let mut setup = self.setup.lock().unwrap();
        if setup.enabled == enabled {
            return;
        }
        setup.enabled = enabled;
        setup.cancel.cancel();
        setup.cancel = CancellationToken::new();
        setup.status = ModelStatus::new(if !enabled {
            "idle"
        } else if setup.ready {
            "ready"
        } else {
            "preparing"
        });
        if enabled && !setup.ready {
            self.queue_setup(setup.cancel.clone());
        }
    }

    pub fn retry(self: &Arc<Self>) -> Result<(), String> {
        let mut setup = self.setup.lock().unwrap();
        if !setup.enabled {
            return Err("Turn on Text Extractor first.".into());
        }
        if setup.status.phase != "error" {
            return Ok(());
        }
        setup.cancel.cancel();
        setup.cancel = CancellationToken::new();
        setup.status = ModelStatus::new("preparing");
        self.queue_setup(setup.cancel.clone());
        Ok(())
    }

    fn queue_setup(self: &Arc<Self>, cancel: CancellationToken) {
        let this = self.clone();
        tauri::async_runtime::spawn(async move {
            // Re-enabling or retrying cannot race another writer of the same files.
            let _guard = this.preparing.lock().await;
            if cancel.is_cancelled() {
                return;
            }
            let result = this.prepare(&cancel).await;
            let mut setup = this.setup.lock().unwrap();
            if cancel.is_cancelled() {
                return;
            }
            match result {
                Ok(engine) => {
                    *this.engine.lock().unwrap() = Some(engine);
                    setup.ready = true;
                    setup.status = ModelStatus::new("ready");
                    eprintln!("Offline OCR models are ready.");
                }
                Err(error) => {
                    eprintln!("Offline OCR setup: {error}");
                    setup.status = ModelStatus::new("error");
                    setup.status.error = Some(error);
                }
            }
        });
    }

    fn update(&self, cancel: &CancellationToken, status: ModelStatus) {
        let mut setup = self.setup.lock().unwrap();
        if !cancel.is_cancelled() {
            setup.status = status;
        }
    }

    async fn prepare(&self, cancel: &CancellationToken) -> Result<Ocr, String> {
        tokio::fs::create_dir_all(&self.directory)
            .await
            .map_err(|_| {
                "Could not create the offline model folder. Check available disk space."
            })?;
        // Building a client does not contact the network. Valid cached files work offline.
        let client = reqwest::Client::builder()
            .https_only(true)
            // ModelScope's CDN rejects an absent User-Agent with HTTP 403.
            .user_agent(concat!("Pulse/", env!("CARGO_PKG_VERSION")))
            .redirect(reqwest::redirect::Policy::limited(5))
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(180))
            .build()
            .map_err(|_| "Could not prepare the model download.")?;
        for (index, model) in MODELS.iter().enumerate() {
            let path = self.directory.join(model.name);
            tokio::select! {
                _ = cancel.cancelled() => return Err("Download cancelled.".into()),
                result = async {
                    if valid_file(&path, model.sha256).await? { return Ok(()); }
                    self.download(&client, model, index + 1, &path, cancel).await
                } => result?,
            }
        }
        if cancel.is_cancelled() {
            return Err("Download cancelled.".into());
        }
        self.update(cancel, ModelStatus::new("loading"));
        let detector = self.directory.join(MODELS[0].name);
        let recognizer = self.directory.join(MODELS[1].name);
        // Load sessions once; no dummy inference or benchmark is performed.
        tauri::async_runtime::spawn_blocking(move || Ocr::load(&detector, &recognizer))
            .await
            .map_err(|_| "Could not load offline text recognition. Retry setup.".to_string())?
    }

    async fn download(
        &self,
        client: &reqwest::Client,
        model: &Model,
        index: usize,
        path: &Path,
        cancel: &CancellationToken,
    ) -> Result<(), String> {
        let mut failures = Vec::new();
        for source in &model.sources {
            if cancel.is_cancelled() {
                return Err("Download cancelled.".into());
            }
            match self
                .download_from(client, model, source, index, path, cancel)
                .await
            {
                Ok(()) => return Ok(()),
                Err(DownloadError::Local(message)) => return Err(message.into()),
                Err(DownloadError::Remote(reason)) => {
                    eprintln!(
                        "Offline model {} from {}: {reason}",
                        model.name, source.name
                    );
                    failures.push(format!("{}: {reason}", source.name));
                }
            }
        }
        Err(format!(
            "Could not download offline OCR ({}). Retry setup.",
            failures.join("; ")
        ))
    }

    async fn download_from(
        &self,
        client: &reqwest::Client,
        model: &Model,
        source: &ModelSource,
        index: usize,
        path: &Path,
        cancel: &CancellationToken,
    ) -> Result<(), DownloadError> {
        let mut status = ModelStatus::new("downloading");
        status.model_index = index;
        self.update(cancel, status.clone());
        let partial = path.with_extension("onnx.part");
        let result = async {
            let mut response = client
                .get(source.url)
                .send()
                .await
                .map_err(connection_error)?;
            if !response.status().is_success() {
                return Err(DownloadError::Remote(format!(
                    "HTTP {}",
                    response.status().as_u16()
                )));
            }
            let total = response.content_length();
            if total.is_some_and(|size| size == 0 || size > MAX_MODEL_BYTES) {
                return Err(DownloadError::Remote("unexpected file size".into()));
            }
            status.total_bytes = total;
            let mut file = tokio::fs::File::create(&partial).await.map_err(|_| {
                DownloadError::Local(
                    "Could not save the offline model. Check available disk space.",
                )
            })?;
            let mut hash = Sha256::new();
            while let Some(chunk) = response.chunk().await.map_err(connection_error)? {
                status.downloaded_bytes += chunk.len() as u64;
                if status.downloaded_bytes > MAX_MODEL_BYTES {
                    return Err(DownloadError::Remote("unexpected file size".into()));
                }
                hash.update(&chunk);
                file.write_all(&chunk).await.map_err(|_| {
                    DownloadError::Local(
                        "Could not save the offline model. Check available disk space.",
                    )
                })?;
                self.update(cancel, status.clone());
            }
            if format!("{:x}", hash.finalize()) != model.sha256 {
                return Err(DownloadError::Remote("file checksum mismatch".into()));
            }
            file.flush()
                .await
                .map_err(|_| DownloadError::Local("Could not finish saving the offline model."))?;
            file.sync_all()
                .await
                .map_err(|_| DownloadError::Local("Could not finish saving the offline model."))?;
            drop(file);
            // Windows rename cannot replace an existing corrupted cache file.
            if tokio::fs::try_exists(path)
                .await
                .map_err(|_| DownloadError::Local("Could not read the model folder."))?
            {
                tokio::fs::remove_file(path).await.map_err(|_| {
                    DownloadError::Local("Could not replace the damaged offline model.")
                })?;
            }
            tokio::fs::rename(&partial, path)
                .await
                .map_err(|_| DownloadError::Local("Could not install the offline model."))?;
            Ok(())
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&partial).await;
        }
        result
    }

    pub fn recognize(
        &self,
        image: RgbaImage,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        {
            let setup = self.setup.lock().unwrap();
            if !setup.enabled {
                return Err("Text Extractor is turned off.".into());
            }
            if !setup.ready {
                return Err(setup.status.error.clone().unwrap_or_else(||
                    "Offline text recognition is getting ready. Check its progress in Settings → Advanced, then try again.".into()));
            }
        }
        // Holding this guard outside catch_unwind keeps a library panic from poisoning
        // the shared lock. A cancelled queued capture is discarded before inference.
        let mut engine = self.engine.lock().unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            engine
                .as_mut()
                .ok_or("Offline text recognition is unavailable.")?
                .recognize(image, cancel)
        }));
        match result {
            Ok(result) => result,
            Err(_) => {
                *engine = None;
                drop(engine);
                let mut setup = self.setup.lock().unwrap();
                setup.ready = false;
                setup.status = ModelStatus::new("error");
                let error = "Offline text recognition stopped unexpectedly. Retry setup in Settings → Advanced.".to_string();
                setup.status.error = Some(error.clone());
                Err(error)
            }
        }
    }
}

async fn valid_file(path: &Path, expected_hash: &str) -> Result<bool, String> {
    let mut file = match tokio::fs::File::open(path).await {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => {
            return Err(
                "Could not read the offline model. Check access to the model folder.".into(),
            )
        }
    };
    let size = file
        .metadata()
        .await
        .map_err(|_| "Could not read the offline model.")?
        .len();
    if size == 0 || size > MAX_MODEL_BYTES {
        return Ok(false);
    }
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut read = 0;
    loop {
        let count = file
            .read(&mut buffer)
            .await
            .map_err(|_| "Could not read the offline model.")?;
        if count == 0 {
            break;
        }
        read += count as u64;
        if read > MAX_MODEL_BYTES {
            return Ok(false);
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()) == expected_hash)
}
