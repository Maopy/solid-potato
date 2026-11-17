# Third-party notices

## Tauri desktop client

The Windows desktop client uses [Tauri](https://tauri.app/), available under
the MIT or Apache-2.0 license. The Tauri MIT license is included as
`licenses/Tauri-MIT.txt`; the Apache-2.0 license is also included.

## YAML configuration editor

The application uses [yaml-edit](https://github.com/jelmer/yaml-edit),
licensed under Apache License 2.0. The license is included as
`licenses/Apache-2.0.txt`.

## Historical provenance: wx_channels_download

Earlier development used
[`ltaoo/wx_channels_download`](https://github.com/ltaoo/wx_channels_download)
at commit `2bcc78bd1641985405f362226a9dc9b50eb2b3cd` as a behavioral reference
for the WeChat Channels capture flow.

The complete source tree is not distributed in this repository. The exact
browser resources needed to preserve the existing WeChat Channels recording
flow are retained under `assets/wechat_capture/`; the Go service, build tools,
documentation, and unrelated downloader/decryption features are omitted.

The historical project is licensed under the MIT License with Commons Clause
License Condition v1.0, copyright (c) 2025 ltaoo. The repository-level
[LICENSE](LICENSE) retains the applicable notice for any adapted work.

## Optional StartLux decision runtime

The local comment scorer uses [StartLuxLabs/StartLux-Decision](https://github.com/StartLuxLabs/StartLux-Decision)
(Apache-2.0 inference code) and the official StartLux-Decision-v1.1-0.8B-Instant
weights (CC BY-NC 4.0). Commercial use of these weights requires a separate
license from StartLux Labs. The model, upstream inference code and Python/CUDA
dependencies are installed separately and are not included in the release ZIP.
The local Windows adapter permits PyTorch's mathematical SDPA kernel while
preserving the official bidirectional prompt mask and all startup self-checks.

## Optional FunASR runtime

The local speech pipeline uses [FunASR](https://github.com/modelscope/FunASR)
and its Paraformer, [FSMN-VAD](https://huggingface.co/funasr/fsmn-vad), and
[CT-Transformer punctuation](https://huggingface.co/funasr/ct-punc), and [CAM++ speaker verification](https://huggingface.co/funasr/campplus) models.
The runtime and model weights are installed separately from the release archive;
their upstream licenses apply.
