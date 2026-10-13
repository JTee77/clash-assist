/**
 * 每个 error.code 对应一句「下一步怎么做」（中文、可执行）。
 * errEnvelope 自动把它放进 error.hint。docs/CLI.md「错误码」表与此同步（cli.test.ts 断言）。
 */
export const CLI_ERROR_HINTS: Record<string, string> = {
  unknown_command: "运行 clash-assist --cli help --json 查看全部命令。",
  unknown_option: "运行 clash-assist --cli help --json 查看每个命令支持的选项。",
  missing_option_value: "在选项后面补上值，例如 --client verge、--limit 3。",
  unexpected_argument: "去掉多余参数；节点名含空格请加引号，或用 check nodes \"A\" \"B\"。",
  option_not_applicable:
    "--region/--match/--limit 只用于 nodes、check all、check nodes；--dir 只用于 install-shim；--port 只用于 serve。",
  invalid_limit: "--limit 需要正整数，例如 --limit 3。",
  node_name_required: '给出节点名，例如 check node "香港 01"；先用 nodes 命令查看完整名称。',
  client_required: "加上 --client verge（目前只支持 Clash Verge），或加 --mock 试跑。",
  client_unsupported: "改用 --client verge；FlClash 等其它软件暂不支持。",
  nodes_unavailable: "确认 Clash Verge 已打开且已加载订阅，再运行 discover 看 status / nodesError。",
  node_not_found: "运行 nodes（可加 --match 关键词）查看准确节点名后重试。",
  node_ambiguous: "写出完整节点名（nodes 命令可列出全部名称），或用 --match 配合 check all --limit。",
  no_nodes_matched: "放宽 --region / --match，或先运行 nodes 带同样筛选条件确认能匹配到节点。",
  gate_failed: "先确认 Clash Verge 已开启系统代理/TUN 且能访问海外网站，再运行 gate 复查。",
  check_failed: "看 data.hint 定位原因；确认策略组可手动切换（Selector），必要时先 check current。",
  restore_failed: "请立刻到 Clash Verge 里手动选回原节点（见 message 里的节点名），再重试。",
  aborted: "检测被中断，结果不完整；如切换过节点，请确认 data.restoreError 为空或手动选回。",
  os_unsupported: "需要 macOS 14 或更高版本才能在 CLI 隐藏窗里跑 check/env；请升级系统或改用图形界面。",
  dispatch_error: "未预期异常：重试一次；仍失败请带上完整 JSON 反馈问题。",
  boot_error: "CLI 启动异常：确认运行的是完整应用二进制（或 pnpm tauri dev），重试一次。",
  argv_unavailable: "CLI 启动异常：读不到命令行参数，请直接运行应用二进制重试。",
  invalid_port: "--port 需要 0–65535 的整数（0 = 临时端口）。",
  port_only_for_serve: "去掉 --port，或改为 clash-assist --cli serve --port 17890。",
  serve_bind_failed: "端口可能被占用：换 --port（或 --port 0 让系统分配）。",
  serve_not_oneshot: "serve 请直接以 clash-assist --cli serve 启动，不要经 HTTP 调用。",
  serve_handler_error: "serve 处理请求时异常：重试一次；仍失败请重启 serve。",
  invalid_body: '请求体必须是 JSON 对象，例如 {"name":"香港 01"} 或 {"limit":2,"region":"日本"}。',
  unknown_route: "使用文档列出的路由：POST /v1/discover|nodes|gate|env|check/current|check/all|check/node。",
  shim_exists: "目标位置已有同名文件且不是本工具生成：用 --dir 换目录，或自行确认后删除该文件再重试。",
  shim_failed: "用 --dir 指定一个可写目录（如 ~/.local/bin），或直接使用应用内完整路径。",
  shim_unavailable: "install-shim 只能在应用二进制里运行（不支持 serve / 纯 Node 环境）。",
  update_check_failed: "检查网络（或代理）能否访问 api.github.com 后重试；也可直接打开 https://github.com/JTee77/clash-assist/releases 。",
  update_check_unavailable: "update-check 只能在应用二进制里运行。",
  // serve（Rust 直接返回的 JSON 里也带同一句 hint）
  unauthorized: "token 在 serve 启动时打印（stdout 横幅 data.token），每次启动都会变；请用最新 token。",
  not_found: "检查路径拼写；可用 clash-assist --cli help 查看全部路由。",
  method_not_allowed: "业务路由都用 POST；健康检查用 GET /health。",
  timeout: "check all 节点多时会很久：用 --limit / --region 缩小范围，或改用一次性 CLI。",
  server_stopping: "serve 正在退出，稍后重新启动 clash-assist --cli serve 再请求。",
};

export function hintFor(code: string): string | undefined {
  return CLI_ERROR_HINTS[code];
}
