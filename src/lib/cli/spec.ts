/**
 * 机器可读的命令/选项清单（`help --json` 的 data.spec）。
 * 给 AI 助手 / 脚本用：不用解析 help 文本。与 parse.ts / docs/CLI.md 同步（cli.test.ts 断言命令全集一致）。
 */
import { CLI_ERROR_HINTS } from "./hints";

export type CliSpecOption = {
  name: string;
  alias?: string;
  value?: string;
  appliesTo: string[] | "all";
  description: string;
};

export type CliSpecCommand = {
  name: string;
  usage: string;
  description: string;
  /** 会切换代理节点（结束时切回） */
  switchesNodes: boolean;
  /** 走代理真实发请求、耗流量（含带宽抽样） */
  costly: boolean;
  /** 会往 stderr 写 NDJSON 进度 */
  progress: boolean;
  /** 建议给调用方设置的超时（秒） */
  suggestedTimeoutSec: number;
  serveRoute: string | null;
};

const FILTERS = ["nodes", "check all", "check nodes"];

export const CLI_SPEC = {
  schema: 1,
  invocation: {
    appBinary: "/Applications/Clash Assist.app/Contents/MacOS/clash-assist",
    shim: "clash-assist（运行 install-shim 后可用，等价于 <appBinary> --cli）",
    prefix: "--cli",
  },
  stdout: "一行 CliEnvelope JSON：{ ok, version, command, ranAt, data? , error?: { code, message, hint } }",
  stderr: "check / env 运行时每行一个 JSON 进度事件（type=start|stage|card|node_done|done）；--no-progress 关闭。其它人类文案不以 { 开头。",
  exitCodes: { "0": "ok:true", "1": "ok:false", "130": "强制中断（无 envelope）" },
  commands: [
    { name: "help", usage: "help [--json]", description: "说明；data.text 为文本，data.spec 为本清单", switchesNodes: false, costly: false, progress: false, suggestedTimeoutSec: 15, serveRoute: null },
    { name: "discover", usage: "discover --client verge", description: "发现控制器、连接状态、当前节点、节点摘要（最多 50 个）", switchesNodes: false, costly: false, progress: false, suggestedTimeoutSec: 30, serveRoute: "POST /v1/discover" },
    { name: "nodes", usage: "nodes --client verge [--region R] [--match S] [--limit N]", description: "全部节点（不截断），可筛选", switchesNodes: false, costly: false, progress: false, suggestedTimeoutSec: 30, serveRoute: "POST /v1/nodes" },
    { name: "gate", usage: "gate --client verge", description: "轻量门槛：是否连上、是否像没走代理", switchesNodes: false, costly: false, progress: false, suggestedTimeoutSec: 45, serveRoute: "POST /v1/gate" },
    { name: "check current", usage: "check current --client verge", description: "当前出口完整深测（不切换）", switchesNodes: false, costly: true, progress: true, suggestedTimeoutSec: 180, serveRoute: "POST /v1/check/current" },
    { name: "check node", usage: "check node <名称> --client verge", description: "指定节点完整深测（临时切换，测完切回）", switchesNodes: true, costly: true, progress: true, suggestedTimeoutSec: 240, serveRoute: "POST /v1/check/node {name}" },
    { name: "check nodes", usage: "check nodes <名称1> <名称2>… --client verge [--limit N]", description: "多个指定节点逐个深测并切回（输出形状同 check all）", switchesNodes: true, costly: true, progress: true, suggestedTimeoutSec: 240, serveRoute: "POST /v1/check/all {names}" },
    { name: "check all", usage: "check all --client verge [--region R] [--match S] [--limit N]", description: "全部（或筛选后）节点逐个深测并切回；每个节点约 1–3 分钟", switchesNodes: true, costly: true, progress: true, suggestedTimeoutSec: 240, serveRoute: "POST /v1/check/all {region,match,limit}" },
    { name: "env", usage: "env --client verge", description: "环境泄漏检查（DNS / IPv6 / WebRTC / 分流 / 直连旁路）", switchesNodes: false, costly: false, progress: true, suggestedTimeoutSec: 120, serveRoute: "POST /v1/env" },
    { name: "serve", usage: "serve --client verge [--port 17890]", description: "常驻 127.0.0.1 HTTP（token 鉴权）", switchesNodes: false, costly: false, progress: false, suggestedTimeoutSec: 0, serveRoute: null },
    { name: "install-shim", usage: "install-shim [--dir D]", description: "写 clash-assist 包装脚本到 /usr/local/bin（可写时）或 ~/.local/bin", switchesNodes: false, costly: false, progress: false, suggestedTimeoutSec: 15, serveRoute: null },
    { name: "update-check", usage: "update-check", description: "查询 GitHub 最新 release；data = { current, latest, update_available, url }。网络失败 → update_check_failed", switchesNodes: false, costly: false, progress: false, suggestedTimeoutSec: 15, serveRoute: null },
  ] satisfies CliSpecCommand[],
  options: [
    { name: "--client", alias: "-c", value: "verge|flclash", appliesTo: ["discover", "nodes", "gate", "check", "env", "serve"], description: "支持 verge（Clash Verge）或 flclash（FlClash）" },
    { name: "--mock", appliesTo: "all", description: "演示节点，不连真实软件（探针仍真实发请求）" },
    { name: "--region", value: "地区[,地区]", appliesTo: FILTERS, description: "按地区筛选：中文名或别名（HK、JP、🇯🇵…）" },
    { name: "--match", value: "关键词[,关键词]", appliesTo: FILTERS, description: "节点名包含任一关键词（不区分大小写）" },
    { name: "--limit", alias: "-n", value: "N", appliesTo: FILTERS, description: "筛选后最多取前 N 个" },
    { name: "--cached", appliesTo: ["check", "env"], description: "读取本地缓存的最近一次检测结果（与 GUI 互通，不发探针、不切节点）" },
    { name: "--no-progress", alias: "--quiet / -q", appliesTo: "all", description: "不写 stderr NDJSON 进度" },
    { name: "--port", alias: "-p", value: "N", appliesTo: ["serve"], description: "监听端口（默认 17890；0 = 临时）" },
    { name: "--dir", value: "绝对路径", appliesTo: ["install-shim"], description: "shim 安装目录" },
    { name: "--json", appliesTo: "all", description: "一行 JSON（默认）" },
    { name: "--no-json", appliesTo: "all", description: "人类可读摘要" },
    { name: "--help", alias: "-h", appliesTo: "all", description: "同 help" },
  ] satisfies CliSpecOption[],
  errorCodes: CLI_ERROR_HINTS,
  recommendedSequence: [
    "discover --client verge（看 status / currentProxy）",
    "gate --client verge（data.gate.ok 为 true 再继续）",
    "nodes --client verge [--region …]（选节点）",
    "check current | check node <名> | check all --limit N",
  ],
} as const;
