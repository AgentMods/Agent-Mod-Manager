import { useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { FileTextIcon, ImageIcon, LoaderCircleIcon, RotateCcwIcon, XIcon } from "lucide-react";
import { ErrorState, Modal } from "./ui";
import { Button } from "./ui/button";
import { Spinner } from "./ui/spinner";
import { McpLaunchDetails } from "./McpLaunchDetails";
import { useBackendQuery } from "@/lib/api";
import { startMcpConnection, type McpConfig } from "@/lib/mcp-connection";
import { Attachment, AttachmentAction, AttachmentActions, AttachmentContent, AttachmentDescription, AttachmentMedia, AttachmentTitle, AttachmentTrigger } from "./ui/attachment";
import type { AgentAttachment } from "@/lib/types";
import { imageType, type DraftAttachment } from "@/lib/agent-attachments";

function AttachmentCard({ name, image, value, error, actions }: {
  name: string; image: boolean; value?: AgentAttachment; error?: string; actions?: ReactNode;
}) {
  const [preview, setPreview] = useState(false);
  const status = error || (!value ? (image ? "正在读取图片…" : "MarkItDown 正在转换…") : value.kind === "document" ? "Markdown" : undefined);
  return <Attachment state={error ? "error" : value ? "done" : "processing"} size="xs"
    className="max-w-full min-w-0 flex-nowrap gap-2 text-left font-normal select-text">
    <AttachmentMedia variant={value?.kind === "image" ? "image" : "icon"}>
      {value?.kind === "image" ? <img src={value.dataUrl} alt={name} /> : !value && !error
        ? <LoaderCircleIcon aria-hidden="true" className="animate-spin" /> : image ? <ImageIcon aria-hidden="true" /> : <FileTextIcon aria-hidden="true" />}
    </AttachmentMedia>
    <AttachmentContent className={error ? "max-w-sm" : "max-w-56"}>
      <AttachmentTitle title={name}>{name}</AttachmentTitle>
      {status && <AttachmentDescription role={error ? "alert" : undefined} className={error ? "whitespace-normal break-words" : undefined}>{status}</AttachmentDescription>}
    </AttachmentContent>
    {value && <AttachmentTrigger aria-label={`预览 ${name}`} title={`预览 ${name}`} className="rounded-lg" onClick={() => setPreview(true)} />}
    {actions && <AttachmentActions>{actions}</AttachmentActions>}
    {preview && value && <Modal title={name} description={value.kind === "document" ? "MarkItDown 转换结果" : undefined} onClose={() => setPreview(false)}>
      {value.kind === "image" ? <img src={value.dataUrl} alt={name} className="max-h-[75dvh] self-center object-contain" />
        : <pre className="text-sm break-words whitespace-pre-wrap">{value.markdown}</pre>}
    </Modal>}
  </Attachment>;
}

export function MessageAttachment({ value }: { value: AgentAttachment }) {
  return <AttachmentCard name={value.name} image={value.kind === "image"} value={value} />;
}

export function AttachmentConverterSetup({ onClose, onStarted }: { onClose: () => void; onStarted: () => void }) {
  const configs = useBackendQuery<McpConfig[]>(["mcp", "configs"], "mcp_list");
  const config = configs.data?.find((entry) => entry.id === "markitdown" && entry.builtin && entry.transport === "stdio");
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState<unknown>();
  const client = useQueryClient();
  async function start() {
    if (!config || starting) return;
    setStarting(true); setError(null);
    try {
      await startMcpConnection(config, true);
      void client.invalidateQueries({ queryKey: ["mcp"] });
      onStarted();
    } catch (failure) { setError(failure); }
    finally { setStarting(false); }
  }
  return <Modal title="启动 MarkItDown MCP" onClose={() => { if (!starting) onClose(); }} className="sm:max-w-md" actions={<>
    <Button type="button" variant="outline" disabled={starting} onClick={onClose}>取消</Button>
    <Button type="button" disabled={!config || starting} onClick={() => void start()}>{starting && <Spinner />}启动并转换</Button>
  </>}>
    {config ? <McpLaunchDetails config={config} /> : configs.isLoading ? <Spinner /> : null}
    <ErrorState error={error || configs.error || (configs.data && !config ? "请在设置的 MCP 服务中恢复内置 MarkItDown 预设。" : null)} />
  </Modal>;
}

export function ComposerAttachment({ draft, disabled, onRemove, onRetry }: {
  draft: DraftAttachment; disabled: boolean; onRemove: () => void; onRetry: () => void;
}) {
  const ready = draft.state === "ready";
  const name = ready ? draft.attachment.name : draft.file.name;
  return <AttachmentCard name={name} image={ready ? draft.attachment.kind === "image" : !!imageType(draft.file)}
    value={ready ? draft.attachment : undefined} error={draft.state === "error" ? draft.error : undefined}
    actions={<>
      {draft.state === "error" && !draft.needsConsent && <AttachmentAction type="button" aria-label={`重试 ${name}`} title="重试" disabled={disabled} onClick={onRetry}>
        <RotateCcwIcon aria-hidden="true" />
      </AttachmentAction>}
      <AttachmentAction type="button" aria-label={`移除 ${name}`} title="移除附件" disabled={disabled} onClick={onRemove}><XIcon aria-hidden="true" /></AttachmentAction>
    </>} />;
}
