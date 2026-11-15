/**
 * 同一句说明只保留一次。
 * 刷新节点失败时，门槛的 message 和 process 经常是同一句「请打开客户端 / 开启外部控制器」。
 */

export function distinctMessages(parts: readonly string[]): string {
  const seen = new Set<string>();
  const lines: string[] = [];
  for (const part of parts) {
    for (const raw of part.split(/\r?\n/)) {
      const text = raw.trim();
      if (!text || seen.has(text)) continue;
      seen.add(text);
      lines.push(text);
    }
  }
  return lines.join("\n");
}
