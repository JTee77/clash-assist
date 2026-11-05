//! `updater-e2e` 测试构建标记（windows-plan r4 §2.4「测试专用 feature」第 3 点，M-7 / S-R4）。
//!
//! 只在 `--features updater-e2e` 时编译（`lib.rs` 中 `#[cfg(feature = "updater-e2e")] mod e2e_marker;`）。
//! release workflow 绝不启用该 feature：`release-windows.yml` 用 `scan-exe-markers.ps1` 反向断言 exe 中
//! **不含**这串字节，`updater-e2e.yml` 用 `-RequireE2EMarker` 正向断言 e2e 构建**必须含**，两者一起才算有效。
//!
//! 这里只有标记本身；端点覆盖（`CLASH_ASSIST_E2E_API`）与下载白名单放宽属于更新器核心（M5，Pit）。

/// 字节数组本身就是标记内容（release profile `strip = true` 会去掉符号名，所以标记必须在数据里）。
/// `#[used]`：即使没有代码路径读取也保留在目标文件中。
#[used]
static CLASH_ASSIST_UPDATER_E2E_BUILD: [u8; 30] = *b"CLASH_ASSIST_UPDATER_E2E_BUILD";

/// 启动时调用一次：经 `black_box` 引用标记，防止 LTO 把它当成未使用的数据删掉。
pub fn touch() {
    std::hint::black_box(&CLASH_ASSIST_UPDATER_E2E_BUILD);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_bytes_match_scanner_needle() {
        // 与 scripts/scan-exe-markers.ps1 的 $AlwaysBad 一致
        assert_eq!(
            &CLASH_ASSIST_UPDATER_E2E_BUILD,
            b"CLASH_ASSIST_UPDATER_E2E_BUILD"
        );
        touch();
    }
}
