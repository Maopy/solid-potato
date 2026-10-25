# Third-party notices

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

## Optional FireRedASR2S runtime

The optional local speech adapter uses
[`FireRedTeam/FireRedASR2S`](https://github.com/FireRedTeam/FireRedASR2S),
licensed under Apache License 2.0, and the FireRedASR2-AED checkpoint from its
official model repository. The upstream runtime, model weights, Python and
PyTorch/CUDA packages are installed separately and are not included in this
release archive. Their upstream licenses continue to apply.
