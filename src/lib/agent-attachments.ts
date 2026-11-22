import { useEffect } from "react";
import { api, errorText } from "./api";
import type { AgentAttachment } from "./types";

export type DraftAttachment =
  | { id: string; state: "processing"; file: File }
  | { id: string; state: "error"; file: File; error: string; needsConsent?: boolean }
  | { id: string; state: "ready"; attachment: AgentAttachment };

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

function imageDataUrl(blob: Blob, signal: AbortSignal): Promise<string> {
  return new Promise((resolve, reject) => {
    signal.throwIfAborted();
    const reader = new FileReader();
    const abort = () => reader.abort();
    signal.addEventListener("abort", abort, { once: true });
    reader.onabort = () => reject(signal.reason);
    reader.onloadend = () => signal.removeEventListener("abort", abort);
    reader.onerror = () => reject(new Error("图片读取失败，请重新选择文件。"));
    reader.onload = () => resolve(reader.result as string);
    reader.readAsDataURL(blob);
  });
}

export function readyAttachment(attachment: AgentAttachment, id: string = crypto.randomUUID()): DraftAttachment {
  return { id, state: "ready", attachment };
}

async function prepareAttachment(file: File, id: string, signal: AbortSignal): Promise<DraftAttachment> {
  const type = imageType(file);
  if (!file.size) throw new Error("附件为空，请选择包含内容的文件。");
  if (type) {
    if (!nativeImageFormats.has(type)) throw new Error("请将图片另存为 PNG、JPEG、WebP 或 GIF 后添加。");
    const dataUrl = await imageDataUrl(file.type === type ? file : file.slice(0, file.size, type), signal);
    signal.throwIfAborted();
    return readyAttachment({ kind: "image", name: file.name, dataUrl }, id);
  }
  // A fresh ID identifies an attempt, not the row (which can be retried).
  const bytes = await file.arrayBuffer();
  signal.throwIfAborted();
  const conversionId = crypto.randomUUID();
  await api("begin_agent_attachment", { conversionId });
  let cancelSent = false;
  const cancel = () => {
    if (cancelSent) return;
    cancelSent = true;
    void api("cancel_agent_attachment", { conversionId }).catch(() => { /* webview may be closing */ });
  };
  // Registration may have completed after the row was removed. Do not send
  // bytes until it is acknowledged; cancelling a reservation prevents any late upload.
  signal.addEventListener("abort", cancel, { once: true });
  try {
    signal.throwIfAborted();
    const markdown = await api<string>("convert_agent_attachment", bytes, {
      headers: {
        "x-attachment-name": encodeURIComponent(file.name),
        "x-attachment-conversion-id": conversionId,
      },
    });
    signal.throwIfAborted();
    if (!markdown.trim()) throw new Error("未从附件中读取到文本，请检查文件内容。");
    return readyAttachment({ kind: "document", name: file.name, markdown }, id);
  } catch (failure) {
    // If transport failed before the upload reached Rust, release its reservation.
    cancel();
    throw failure;
  } finally {
    signal.removeEventListener("abort", cancel);
  }
}

/** Removing the active row or unloading the draft cancels this attempt. */
export function usePrepareAttachments(attachments: DraftAttachment[], update: (apply: (current: DraftAttachment[]) => DraftAttachment[]) => void) {
  const next = attachments.find((item) => item.state === "processing");
  useEffect(() => {
    if (!next) return;
    const { id, file } = next;
    const controller = new AbortController();
    const { signal } = controller;
    const abort = () => controller.abort();
    window.addEventListener("pagehide", abort, { once: true });
    async function prepare() {
      let result: DraftAttachment;
      try { result = await prepareAttachment(file, id, signal); }
      catch (failure) {
        if (signal.aborted) return;
        const cause = failure instanceof Error ? failure.cause : null;
        const needsConsent = !!cause && typeof cause === "object" && "code" in cause && cause.code === "consent_required";
        result = { id, state: "error", file, needsConsent, error: needsConsent ? "等待启动 MarkItDown MCP" : errorText(failure) };
      }
      if (!signal.aborted) update((current) => current.map((item) => item === next ? result : item));
    }
    void prepare();
    return () => {
      window.removeEventListener("pagehide", abort);
      abort();
    };
  }, [next, update]);
}
