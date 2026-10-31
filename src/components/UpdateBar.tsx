import { useUpdateFlow } from "../hooks/useUpdateFlow";
import { UpdatePill } from "./TitleBar";

/**
 * Windows 更新条（windows-plan §2.7）：Windows 用原生标题栏（标题与版本由 Rust set_title 写入），
 * 内容区顶部只在有更新 / 上次更新未完成时出现一条更新胶囊；无拖拽区、无红绿灯留白。
 */
export function UpdateBar() {
  const flow = useUpdateFlow({ windows: true });
  if (!flow.view.visible) return null;
  return (
    <div className="update-bar">
      <UpdatePill flow={flow} />
    </div>
  );
}
