/**
 * 「发送这份错误报告」出现在哪些错误上。
 *
 * 本地操作说明、连不上、密钥被拒、当前客户端没连上、用户自己中途停下，都不需要开发者看。
 * 程序自己抛出的异常、环境检查或节点测试里预料之外的失败、以及 Windows 诊断里的拦截/警告，需要。
 * 说明级别的诊断提示不带这个按钮。
 * 普通检测卡片的通过/失败是检测结果，不是这个函数的输入。
 */

export type NoticeLevel = "block" | "warn" | "info";

export type ErrorReportSurface =
  | { surface: "fetch"; status: string; message: string }
  | { surface: "switch" | "restore" | "runner"; message: string }
  | { surface: "notice"; level: NoticeLevel };

/** 用户照着界面就能处理的说明，不限于某一款客户端。 */
export function isLocalHowto(message: string): boolean {
  const text = message.replace(/\s+/g, "");
  return [
    /请先(在上方)?选择/,
    /请先打开/,
    /请先点[「"]?刷新节点/,
    /请确认.{0,80}已打开/,
    /外部控制器/,
    /入站认证/,
    /混合(代理)?端口|mixed-port/i,
    /系统代理|TUN/,
    /换个节点/,
    /未检测到正在运行/,
    /连不上.{0,12}VPN/i,
    /再点[「"]?刷新节点/,
    /密钥被.{0,16}拒绝/,
    /控制器密钥不匹配/,
    /订阅已加载/,
  ].some((re) => re.test(message) || re.test(text));
}

/** 失败内容只是「当前客户端没连上」。单独出现的「未连接」不算，避免把超时类错误一并藏掉。 */
export function isNotConnectedFailure(message: string): boolean {
  const text = message.trim();
  if (/not connected/i.test(text)) return true;
  if (/当前客户端/.test(text) && /未连接|没连上|没有连接|尚未连接|未连上/.test(text)) return true;
  if (/^(客户端|当前软件|当前客户端).{0,24}(未连接|没连上|没有连接|尚未连接)/.test(text)) return true;
  if (/未连接当前客户端|没有连接当前客户端/.test(text)) return true;
  return false;
}

/** 用户自己中断检测。切回失败的「没能切回原先节点」不是中断。 */
export function isUserAbort(message: string): boolean {
  const text = message.trim();
  if (/^(已中断|检测被中断|用户取消|用户中止)/.test(text)) return true;
  if (/\b(user aborted|aborted by user)\b/i.test(text)) return true;
  return false;
}

export function shouldOfferErrorReport(input: ErrorReportSurface): boolean {
  if (input.surface === "notice") return input.level === "block" || input.level === "warn";
  if (input.surface === "fetch") {
    if (input.status === "unreachable" || input.status === "unauthorized") return false;
    if (
      isLocalHowto(input.message) ||
      isNotConnectedFailure(input.message) ||
      isUserAbort(input.message)
    ) {
      return false;
    }
    return true;
  }
  if (
    isNotConnectedFailure(input.message) ||
    isUserAbort(input.message) ||
    isLocalHowto(input.message)
  ) {
    return false;
  }
  return true;
}
