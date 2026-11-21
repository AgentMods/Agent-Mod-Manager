import { useEffect, useRef } from "react";
import { api, errorText } from "./api";
import type { AgentAttachment } from "./types";

export type DraftAttachment =
  | { id: string; state: "processing"; file: File }
  | { id: string; state: "error"; file: File; error: string; needsConsent?: boolean }
  | { id: string; state: "ready"; attachment: AgentAttachment; bytes: number };

export const MAX_ATTACHMENTS = 10;
export const MAX_MESSAGE_ATTACHMENT_BYTES = 20 * 1024 * 1024;
const imageTypes: Record<string, string> = {
  png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", jfif: "image/jpeg", gif: "image/gif", webp: "image/webp",
  svg: "image/svg+xml", bmp: "image/bmp", avif: "image/avif", heic: "image/heic", heif: "image/heif", tiff: "image/tiff", tif: "image/tiff", ico: "image/x-icon",
};
const nativeImageFormats = new Set(["image/png", "image/jpeg", "image/webp", "image/gif"]);

export function imageType(file: File): string | undefined {
  const type = file.type.toLowerCase();
  if (type.startsWith("image/")) return type === "image/jpg" ? "image/jpeg" : type;
  if (type && type !== "application/octet-stream") return undefined;
  const extension = /\.([^.]+)$/.exec(file.name)?.[1].toLowerCase();
  return extension && Object.hasOwn(imageTypes, extension) ? imageTypes[extension] : undefined;
}

function imageDataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onerror = () => reject(new Error("图片读取失败，请重新选择文件。"));
    reader.onload = () => resolve(reader.result as string);
    reader.readAsDataURL(blob);
  });
}

export function readyAttachment(attachment: AgentAttachment, id: string = crypto.randomUUID()): DraftAttachment {
  let bytes: number;
  if (attachment.kind === "image") {
    const url = attachment.dataUrl;
    const encodedLength = url.length - url.indexOf(",") - 1;
    bytes = encodedLength / 4 * 3 - (url.endsWith("==") ? 2 : url.endsWith("=") ? 1 : 0);
  } else {
    bytes = new TextEncoder().encode(attachment.markdown).length;
  }
  return { id, state: "ready", attachment, bytes };
}

async function prepareAttachment(file: File, id: string): Promise<DraftAttachment> {
  const type = imageType(file);
  if (!file.size) throw new Error("附件为空，请选择包含内容的文件。");
  if (type) {
    if (!nativeImageFormats.has(type)) throw new Error("请将图片另存为 PNG、JPEG、WebP 或 GIF 后添加。");
    if (file.size > 5 * 1024 * 1024) throw new Error("图片超过 5 MiB，请缩小后重试。");
    const dataUrl = await imageDataUrl(file.type === type ? file : file.slice(0, file.size, type));
    return { id, state: "ready", bytes: file.size, attachment: { kind: "image", name: file.name, dataUrl } };
  }
  if (file.size > 10 * 1024 * 1024) throw new Error("文件超过 10 MiB，请拆分后重试。");
  const markdown = await api<string>("convert_agent_attachment", await file.arrayBuffer(), {
    headers: { "x-attachment-name": encodeURIComponent(file.name) },
  });
  if (!markdown.trim()) throw new Error("MarkItDown 未提取到文本，请检查文件内容。");
  return readyAttachment({ kind: "document", name: file.name, markdown }, id);
}

/** Start only the next file still present in this draft. Late results replace only their original row. */
export function usePrepareAttachments(attachments: DraftAttachment[], update: (apply: (current: DraftAttachment[]) => DraftAttachment[]) => void) {
  const inFlight = useRef(new Set<string>());
  const next = attachments.find((item) => item.state === "processing");
  useEffect(() => {
    if (!next || inFlight.current.has(next.id)) return;
    const { id, file } = next;
    inFlight.current.add(id);
    async function prepare() {
      let result: DraftAttachment;
      try { result = await prepareAttachment(file, id); }
      catch (failure) {
        const cause = failure instanceof Error ? failure.cause : null;
        const needsConsent = !!cause && typeof cause === "object" && "code" in cause && cause.code === "consent_required";
        result = { id, state: "error", file, needsConsent, error: needsConsent ? "等待启动 MarkItDown MCP" : errorText(failure) };
      }
      inFlight.current.delete(id);
      update((current) => current.map((item) => item.id === id ? result : item));
    }
    void prepare();
  }, [next, update]);
}
