import { useEffect } from "react";
import { HomePage } from "./pages/HomePage";
import { TitleBar } from "./components/TitleBar";
import { UpdateBar } from "./components/UpdateBar";
import { useConnection } from "./hooks/useConnection";
import "./styles/app.css";
import { I18nProvider } from "./i18n";
import { applyPlatformAttr } from "./lib/platform";

const isTauriApp = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
// windows-plan §2.7：<html data-platform> 在首帧前写好，CSS 据此区分 Mac / Windows。
const platform = applyPlatformAttr();

function App() {
  return (
    <I18nProvider>
      <AppInner />
    </I18nProvider>
  );
}

function AppInner() {
  // 玻璃地基：仅在 macOS 的 Tauri（透明窗口 + 原生 vibrancy）下启用 .glass，
  // Windows 用不透明窗口 + 原生标题栏；浏览器/预览环境保持不透明，避免透出杂乱背景。
  useEffect(() => {
    if (isTauriApp && platform === "macos") {
      document.documentElement.classList.add("glass");
    }
  }, []);

  const {
    state,
    nodes,
    discoverySources,
    busy,
    forceMock,
    clientId,
    setClientId,
    refresh,
  } = useConnection();

  return (
    <div className="app-shell">
      {isTauriApp && (platform === "windows" ? <UpdateBar /> : <TitleBar />)}
      <main className="main">
        <HomePage
          connection={state}
          busy={busy}
          nodes={nodes}
          discoverySources={discoverySources}
          clientId={clientId}
          onClientIdChange={setClientId}
          onRefresh={(explicitClientId) =>
            refresh(undefined, undefined, explicitClientId)
          }
          forceMock={forceMock}
        />
      </main>
    </div>
  );
}

export default App;
