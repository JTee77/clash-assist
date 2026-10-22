import { useEffect } from "react";
import { HomePage } from "./pages/HomePage";
import { TitleBar } from "./components/TitleBar";
import { useConnection } from "./hooks/useConnection";
import "./styles/app.css";
import { I18nProvider } from "./i18n";

const isTauriApp = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

function App() {
  return (
    <I18nProvider>
      <AppInner />
    </I18nProvider>
  );
}

function AppInner() {
  // 玻璃地基：仅在 Tauri（透明窗口 + 原生 vibrancy）下启用 .glass，
  // 浏览器/预览环境保持不透明，避免透出杂乱背景。
  useEffect(() => {
    if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) {
      document.documentElement.classList.add("glass");
    }
  }, []);

  const {
    state,
    nodes,
    busy,
    forceMock,
    clientId,
    setClientId,
    refresh,
  } = useConnection();

  return (
    <div className="app-shell">
      {isTauriApp && <TitleBar />}
      <main className="main">
        <HomePage
          connection={state}
          busy={busy}
          nodes={nodes}
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
