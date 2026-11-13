/**
 * 把分享文案画成 PNG。文案本身由 diagnostic.renderShareLines 产生，
 * 这里不再读取节点名或 IP。图片在 WebView 里用 canvas 绘制。
 */

const WIDTH = 720;
const PAD = 36;
const FONT_STACK =
  "-apple-system, BlinkMacSystemFont, 'SF Pro Text', 'PingFang SC', 'Hiragino Sans GB', 'Microsoft YaHei', sans-serif";

function wrapLine(ctx: CanvasRenderingContext2D, text: string, max: number): string[] {
  if (!text) return [""];
  const out: string[] = [];
  let cur = "";
  for (const ch of text) {
    const next = cur + ch;
    if (ctx.measureText(next).width > max && cur.length > 0) {
      out.push(cur);
      cur = ch.trim() ? ch : "";
    } else {
      cur = next;
    }
  }
  if (cur) out.push(cur);
  return out.length > 0 ? out : [""];
}

function roundRect(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number,
): void {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}

function colorFor(line: string, title: boolean): string {
  if (line.startsWith("https://")) return "#2563eb";
  if (line.startsWith("非官方") || line.startsWith("Unofficial")) return "#5c6573";
  if (title) return "#1c1f26";
  return "#1c1f26";
}

/** 返回不含 `data:` 前缀的 PNG base64。没有 canvas 时抛错，由界面提示保存失败。 */
export function renderSharePngBase64(lines: readonly string[]): string {
  if (typeof document === "undefined") throw new Error("canvas unavailable");
  const probe = document.createElement("canvas").getContext("2d");
  if (!probe) throw new Error("canvas unavailable");
  const maxText = WIDTH - PAD * 2;
  const planned: { text: string; font: string; title: boolean }[] = [];
  lines.forEach((line, index) => {
    const title = index === 0;
    const font = title ? `600 20px ${FONT_STACK}` : `14px ${FONT_STACK}`;
    probe.font = font;
    for (const text of wrapLine(probe, line, maxText)) {
      planned.push({ text, font, title });
    }
  });
  const lineH = 24;
  const height = PAD * 2 + planned.length * lineH + 12;
  const scale = 2;
  const canvas = document.createElement("canvas");
  canvas.width = WIDTH * scale;
  canvas.height = Math.max(1, height) * scale;
  const ctx = canvas.getContext("2d");
  if (!ctx) throw new Error("canvas unavailable");
  ctx.scale(scale, scale);
  ctx.fillStyle = "#f4f5f7";
  ctx.fillRect(0, 0, WIDTH, height);
  ctx.fillStyle = "#ffffff";
  roundRect(ctx, 16, 16, WIDTH - 32, Math.max(32, height - 32), 12);
  ctx.fill();
  let y = PAD + 8;
  for (const line of planned) {
    ctx.font = line.font;
    ctx.fillStyle = colorFor(line.text, line.title);
    ctx.fillText(line.text, PAD, y);
    y += lineH;
  }
  const url = canvas.toDataURL("image/png");
  const comma = url.indexOf(",");
  if (comma < 0 || !url.slice(comma + 1)) throw new Error("png encode failed");
  return url.slice(comma + 1);
}
