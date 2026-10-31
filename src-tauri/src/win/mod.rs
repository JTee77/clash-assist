//! Windows FFI：采集 §2.2 L1–L3 与 §2.1 规则 4 需要的 OS 事实（windows-plan r4，M1）。
//!
//! 只采集、不判定：判定在 `mihomo::trust`（纯函数，全平台单测）。代码改编自 V5 探针
//! （spike/pipe-probe/src/win，只读参考），去掉了 JSON 报告，换成 `trust` 的事实类型。

pub mod facts;
pub mod identity;
pub mod paths;
pub mod process;
pub mod security;
pub mod sid;
pub mod tcp;
pub mod util;
