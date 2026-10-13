import { useEffect } from "react";
import { HomePage } from "./pages/HomePage";
import { TitleBar } from "./components/TitleBar";
import { useConnection } from "./hooks/useConnection";
import "./styles/app.css";

const isTauriApp = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

function App() {
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
    manual,
    forceMock,
    clientId,
    setClientId,
    updateManual,
    resetManual,
    refresh,
    setForceMock,
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
          onRefresh={() => refresh()}
          manual={manual}
          forceMock={forceMock}
          onChangeManual={updateManual}
          onResetManual={resetManual}
          onForceMockChange={(v) => {
            setForceMock(v);
            void refresh(undefined, v);
          }}
          onRefreshAdvanced={(override) => refresh(override)}
        />
      </main>
    </div>
  );
}

export default App;
