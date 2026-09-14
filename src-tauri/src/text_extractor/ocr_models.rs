use super::ocr::Ocr;
use image::RgbaImage;
use serde::Serialize;
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
        url: "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv5/det/ch_PP-OCRv5_det_mobile.onnx",
        sha256: "4d97c44a20d30a81aad087d6a396b08f786c4635742afc391f6621f5c6ae78ae",
    },
    Model {
        name: "recognizer.onnx",
        url: "https://www.modelscope.cn/models/RapidAI/RapidOCR/resolve/v3.9.2/onnx/PP-OCRv5/rec/cyrillic_PP-OCRv5_rec_mobile.onnx",
        sha256: "90f761b4bfcce0c8c561c0cb5c887b0971d3ec01c32164bdf7374a35b0982711",
    },
];
struct Model {
    name: &'static str,
    url: &'static str,
    sha256: &'static str,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    phase: &'static str,
    model_index: usize,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    error: Option<String>,
}
impl ModelStatus {
    fn new(phase: &'static str) -> Self {
        Self {
            phase,
            model_index: 0,
            downloaded_bytes: 0,
            total_bytes: None,
            error: None,
        }
    }
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
                }
                Err(error) => {
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
        let mut status = ModelStatus::new("downloading");
        status.model_index = index;
        self.update(cancel, status.clone());
        let partial = path.with_extension("onnx.part");
        let result = async {
            let mut response = client
                .get(model.url)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|_| {
                    "Could not download the offline model. Check your connection and retry."
                })?;
            let total = response.content_length();
            if total.is_some_and(|size| size == 0 || size > MAX_MODEL_BYTES) {
                return Err("The model download has an unexpected size. Retry setup.".into());
            }
            status.total_bytes = total;
            let mut file = tokio::fs::File::create(&partial)
                .await
                .map_err(|_| "Could not save the offline model. Check available disk space.")?;
            let mut hash = Sha256::new();
            while let Some(chunk) = response.chunk().await.map_err(|_| {
                "The model download was interrupted. Check your connection and retry."
            })? {
                status.downloaded_bytes += chunk.len() as u64;
                if status.downloaded_bytes > MAX_MODEL_BYTES {
                    return Err("The model download is too large. Retry setup.".into());
                }
                hash.update(&chunk);
                file.write_all(&chunk)
                    .await
                    .map_err(|_| "Could not save the offline model. Check available disk space.")?;
                self.update(cancel, status.clone());
            }
            if format!("{:x}", hash.finalize()) != model.sha256 {
                return Err(
                    "The offline model download is incomplete or damaged. Retry setup.".into(),
                );
            }
            file.flush()
                .await
                .map_err(|_| "Could not finish saving the offline model.")?;
            file.sync_all()
                .await
                .map_err(|_| "Could not finish saving the offline model.")?;
            drop(file);
            // Windows rename cannot replace an existing corrupted cache file.
            if tokio::fs::try_exists(path)
                .await
                .map_err(|_| "Could not read the model folder.")?
            {
                tokio::fs::remove_file(path)
                    .await
                    .map_err(|_| "Could not replace the damaged offline model.")?;
            }
            tokio::fs::rename(&partial, path)
                .await
                .map_err(|_| "Could not install the offline model.")?;
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
