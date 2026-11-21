import { PathText } from "./ui";
import type { McpConfig } from "@/lib/mcp-connection";

export function McpLaunchDetails({ config }: { config: McpConfig }) {
  return <>
    <PathText className="rounded-md border bg-muted p-3">{[config.command, ...config.args].join(" ")}</PathText>
    {Object.keys(config.env_secret_refs).length > 0 && <p className="text-xs text-muted-foreground">环境变量：{Object.keys(config.env_secret_refs).join("、")}</p>}
    <p className="text-sm">该进程会以当前用户的系统权限运行。确认后，相同配置会在下次打开应用时自动启动，直到你手动停止或编辑连接。</p>
    {/^(uvx|uvx\.exe)$/i.test(config.command ?? "") && <p className="text-sm text-muted-foreground">
      使用应用内置的 uvx，默认从清华 PyPI 镜像安装包。首次准备 Python 或下载依赖可能需要数分钟。可在启动参数中用 <code>--default-index</code> 指定其他源，不会更改系统设置。
    </p>}
    {/^(npx|npx\.cmd|npx\.exe)$/i.test(config.command ?? "") && <p className="text-sm text-muted-foreground">
      使用应用内置的 Node.js 和 npx，默认从 npmmirror 安装包。可在包名前用 <code>--registry=地址</code> 指定其他源，不会更改系统设置。
    </p>}
  </>;
}
