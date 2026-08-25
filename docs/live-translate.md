# Live Translate architecture and limitations

## Audio path

```text
physical microphone
        |
        v
Pulse capture -- translation off -----------------> Pulse virtual microphone
        |
        `------ translation on -> OpenAI Realtime Translation -----^
                                                                    |
                                                                    v
                                                           call or chat app
```

While Live Translate is enabled, Pulse continuously routes the selected physical microphone to
the `Pulse` virtual microphone whenever translation is stopped. Starting translation hands the
same virtual endpoint to the translated-audio stream, so receiving applications can keep `Pulse`
selected. The passthrough stream is stopped before translation starts and restored after it stops
or fails; original and translated speech are never mixed.

Pulse captures the selected physical microphone with its native default stream configuration.
Both paths keep the stream's first channel for the entire run instead of changing channels between
audio callbacks. The passthrough path performs band-limited resampling to the virtual device's
native rate and uses a 20 ms buffer for stable,
low-latency routing. The translation path performs band-limited resampling to mono 24 kHz PCM16
and sends continuous 40 ms blocks to the dedicated OpenAI Realtime Translation WebSocket. Pulse
starts local capture while the API session is being prepared, retaining up to two seconds of source
audio so the beginning of the first sentence is not clipped. It configures the selected output
language, enables OpenAI `near_field` input noise reduction, and keeps optional input transcription
disabled. It sends no prompt or instructions. Pulse waits for `session.updated` before sending the
buffered audio so no samples reach the model with the wrong configuration.

Pulse consumes each complete `session.output_audio.delta` directly. It validates the reported
PCM format, sample rate, and channel count, uses 24 kHz directly when the cable supports it,
and otherwise performs band-limited conversion to the cable's native rate. A 200 ms output
prebuffer absorbs the API's variable delta sizes and re-primes after an underrun instead of
playing isolated fragments. There is no transcript-to-speech stage, speaker-monitor path, or
original-microphone mix in the output queue.

The translation session uses:

- model: `gpt-realtime-translate`
- endpoint: `wss://api.openai.com/v1/realtime/translations?model=gpt-realtime-translate`
- client audio: mono 24 kHz PCM16 little-endian
- client block size: 40 ms continuously, including captured silence between phrases
- output languages: English, Spanish, Portuguese, French, Japanese, Russian, Chinese,
  German, Korean, Hindi, Indonesian, Vietnamese, and Italian

The protocol follows the [OpenAI Realtime Translation guide](https://developers.openai.com/api/docs/guides/realtime-translation)
and its [client](https://developers.openai.com/api/reference/resources/realtime/translation-client-events)
and [server](https://developers.openai.com/api/reference/resources/realtime/translation-server-events)
event references.

Pulse keeps the model's native translated audio instead of routing translated transcript text
through a second speech model.

## Why a virtual audio cable is required

A normal desktop application cannot replace the samples produced by an existing hardware
microphone endpoint while preserving that endpoint's identity for every other application.
The operating system exposes audio endpoints owned by hardware or audio drivers. Pulse can
capture from one endpoint and render to another, but it cannot impersonate the original
microphone without installing a virtual audio driver.

On macOS, Pulse bundles its own AudioServer plug-in. It publishes one 48 kHz mono Float32
input stream named `Pulse` and no output stream. The app resamples translated audio to 48 kHz
and sends it over a loopback-only local channel to the driver. Administrator approval remains
required because AudioServer plug-ins live in `/Library/Audio/Plug-Ins/HAL`.

The macOS property and timing implementation is adapted from the MIT-licensed
[Hush](https://github.com/timschmolka/hush) driver; attribution is bundled with the app.

On Windows, Pulse bundles the original [VB-CABLE](https://vb-audio.com/Cable/) archive and
changes the capture endpoint friendly name from `CABLE Output` to `Pulse` without modifying
the signed driver package. The translated render side continues to use the vendor endpoint
internally. There is no output selector or persisted output route in Settings.

Pulse never changes the operating-system default microphone. The receiving application selects
`Pulse` once; stopped translation carries the selected physical microphone, while active
translation carries only the model output. This avoids unexpectedly changing microphone routing
for unrelated apps.

Live Translate is disabled by default. Enabling it installs the platform component and records
Pulse's ownership in the app config directory. On macOS, enabling also migrates the earlier
Pulse-managed VB-CABLE experiment by deleting its aggregate device, driver, and launch daemon
before installing `Pulse.driver`. Disabling first waits for translation to stop, then removes
`Pulse.driver`. On Windows, Pulse removes VB-CABLE only when Pulse installed it. A
restart-required result is shown when the operating system still reports the endpoint after
the lifecycle operation.

## Credentials and local data

The OpenAI API key is stored through the operating system's credential service: macOS
Keychain or Windows Credential Manager. Pulse's JSON translation config stores only the
enabled state, selected language, and physical input-device name.

## Operational limits

- A socket interruption, unexpected `session.closed`, or session expiration reconnects with
  short bounded backoff. Three consecutive failed reconnects stop the session and leave a
  failure message in Settings. A stable connection resets the retry budget.
- Pulse records the current OpenAI session ID, reconnect count, and dropped input-frame count in
  memory. It also logs every session and reconnect. Settings shows these diagnostics while
  translation is active. They are not written to disk and cannot resume or change a model session.
- Temporary input backpressure drops a block instead of killing the session. The capture queue
  holds up to two seconds of audio before it starts dropping new blocks.
- API rejection, microphone loss, or removal of the virtual device stops the session and leaves
  a failure message in Settings.
- Selecting Stop immediately pauses physical microphone capture, requests a graceful
  `session.close`, then restores physical-microphone passthrough when the translation thread exits.
- Output underruns produce silence and re-arm the 200 ms prebuffer. A bounded output queue
  prevents delayed translated speech from growing without limit.
- Device names are persisted. If a device is renamed or disconnected, Pulse requires a new
  selection.
- Pulse rejects known virtual devices as its capture microphone to prevent translated audio
  from feeding back into the translation session.
- macOS now has a Pulse-owned, capture-only driver, so it exposes one `Pulse` input and no
  Pulse output. Windows still uses VB-CABLE, whose render endpoint remains visible because a
  Windows capture-only Pulse driver is not part of this experiment.
- The API is billed to the supplied OpenAI account. Current availability and pricing are
  documented on the [`gpt-realtime-translate` model page](https://developers.openai.com/api/docs/models/gpt-realtime-translate).
