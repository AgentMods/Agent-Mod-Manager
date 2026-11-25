import { open } from "@tauri-apps/plugin-dialog";
import { tr } from "@/lib/i18n";

export function chooseGameProgram(rootPath: string) {
  return open({
    multiple: false,
    directory: false,
    defaultPath: rootPath,
    title: tr("选择游戏启动程序"),
    filters: navigator.userAgent.includes("Windows") ? [{ name: tr("可执行文件"), extensions: ["exe"] }] : undefined,
  });
}
