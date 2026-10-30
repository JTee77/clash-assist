//! Windows 首版不提供 CLI（windows-plan r4 §1 决策 5、§2.5）。
//!
//! `cli_shim` / `cli_serve` 在 Windows 上整体不编译；`lib.rs` 里对应的 Tauri 命令
//! 保留原签名（`generate_handler!` 列表与 `tauriInvokeContract.test.ts` 不变），
//! 但一律返回 [`CLI_UNAVAILABLE`] 错误。本模块只提供这些签名所需的占位类型。
//!
//! 模块在所有平台都编译（非 Windows 上未被使用），以便 macOS / Linux 也能跑它的单测。
#![cfg_attr(not(windows), allow(dead_code))]

/// 所有 CLI 桥命令在 Windows 上的统一错误文案。
pub const CLI_UNAVAILABLE: &str = "Windows 版暂不提供命令行";

/// 成功值占位：Windows 上 CLI 命令永远不会成功，因此是无法构造的空枚举。
#[derive(Debug, Clone, serde::Serialize)]
pub enum Unavailable {}

/// 与 `cli_shim::ShimError` 同形（`{ code, message }`），前端按同一结构解析。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ShimError {
    pub code: String,
    pub message: String,
}

impl ShimError {
    pub fn unavailable() -> Self {
        Self {
            code: "unsupported_platform".into(),
            message: CLI_UNAVAILABLE.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shim_error_shape_matches_unix_variant() {
        let v = serde_json::to_value(ShimError::unavailable()).unwrap();
        assert_eq!(v["code"], "unsupported_platform");
        assert_eq!(v["message"], CLI_UNAVAILABLE);
    }
}
