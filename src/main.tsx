import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { bootCli, shouldBootCli } from "./cli/bootCli";
import { currentLang, format, langFromTag, navigatorTag, readStoredLangPref } from "./i18n";

/** React 树外的兜底文案：按已保存的语言偏好（或系统语言）。 */
function tStatic(key: Parameters<typeof format>[1], vars?: Record<string, string>): string {
  const pref = readStoredLangPref();
  const lang = pref === "system" ? currentLang() ?? langFromTag(navigatorTag()) : pref;
  return format(lang, key, vars);
}

type BoundaryState = { error: Error | null };

class ErrorBoundary extends React.Component<
  { children: React.ReactNode },
  BoundaryState
> {
  state: BoundaryState = { error: null };

  static getDerivedStateFromError(error: Error): BoundaryState {
    return { error };
  }

  render() {
    if (this.state.error) {
      return (
        <div
          style={{
            fontFamily:
              '-apple-system, BlinkMacSystemFont, "PingFang SC", "Helvetica Neue", sans-serif',
            maxWidth: 520,
            margin: "64px auto",
            padding: 24,
            lineHeight: 1.6,
            color: "#e7ecf3",
            background: "#0f1419",
          }}
        >
          <h1 style={{ fontSize: 20, color: "#ffb454", margin: "0 0 12px" }}>
            {tStatic("crash.title")}
          </h1>
          <p>{tStatic("crash.body")}</p>
          <p>
            <code
              style={{
                background: "#1c2430",
                padding: "2px 6px",
                borderRadius: 4,
              }}
            >
              pnpm tauri dev
            </code>
          </p>
          <pre
            style={{
              whiteSpace: "pre-wrap",
              fontSize: 12,
              opacity: 0.75,
              marginTop: 16,
            }}
          >
            {this.state.error.message}
          </pre>
        </div>
      );
    }
    return this.props.children;
  }
}

const rootElOrNull = document.getElementById("root");
if (!rootElOrNull) {
  throw new Error("root element missing");
}
const rootEl: HTMLElement = rootElOrNull;

async function start() {
  try {
    if (await shouldBootCli()) {
      await bootCli();
      return;
    }
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    rootEl.innerHTML = `<div style="font-family:-apple-system,sans-serif;padding:24px;color:#e7ecf3;background:#0f1419"><h1 style="color:#ffb454">CLI 启动失败</h1><pre>${msg.replace(/</g, "&lt;")}</pre></div>`;
    return;
  }

  try {
    ReactDOM.createRoot(rootEl).render(
      <React.StrictMode>
        <ErrorBoundary>
          <App />
        </ErrorBoundary>
      </React.StrictMode>,
    );
  } catch (err) {
    const msg = err instanceof Error ? err.message : String(err);
    rootEl.innerHTML = `<div style="font-family:-apple-system,BlinkMacSystemFont,'PingFang SC',sans-serif;max-width:520px;margin:64px auto;padding:24px;line-height:1.6;color:#e7ecf3;background:#0f1419">
    <h1 style="font-size:20px;color:#ffb454;margin:0 0 12px">${tStatic("boot.title")}</h1>
    <p>${tStatic("boot.body", { cmd: '<code style="background:#1c2430;padding:2px 6px;border-radius:4px">pnpm tauri dev</code>' })}</p>
    <pre style="white-space:pre-wrap;font-size:12px;opacity:.75;margin-top:16px">${msg.replace(/</g, "&lt;")}</pre>
  </div>`;
  }
}

void start();
