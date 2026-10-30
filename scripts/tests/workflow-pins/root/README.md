pins 自测用的迷你仓库根目录（`--repo-root`）。`cases/` 下的用例按这里的目录解析本地引用。
自测脚本会把本目录复制到临时目录，并额外创建符号链接 `.github/actions/comp -> ../../tools/comp`
（不提交符号链接，避免 Windows 检出时出问题）。
