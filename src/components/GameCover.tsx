import { tr } from "@/lib/i18n";
import { useState } from "react";
import { Gamepad2Icon } from "lucide-react";
import { cn } from "@/lib/utils";

// Mirror the manifest cover contract without rewriting CDN paths or query parameters.
function publicCoverUrl(value: unknown): string | null {
  if (typeof value !== "string" || !value.startsWith("https://") || value.length > 2048 || /[\s\\#\p{Cc}]/u.test(value)) return null;
  try {
    const url = new URL(value);
    const host = url.hostname.replace(/\.$/, "");
    if (!host.includes(".") || /^[\d.]+$/.test(host) || /\.(?:localhost|local|internal)$/.test(host) || url.username || url.password || url.port) return null;
    return value;
  } catch { return null; }
}

export function GameCover({ name, url, decorative = false, className }: { name: string; url?: string | null; decorative?: boolean; className?: string }) {
  const [failedUrl, setFailedUrl] = useState<string | null>(null);
  const [loadedUrl, setLoadedUrl] = useState<string | null>(null);
  const src = url !== failedUrl ? publicCoverUrl(url) : null;
  return <div aria-hidden={decorative || undefined} className={cn("group/cover relative flex aspect-[16/8] w-full flex-col items-center justify-center overflow-hidden bg-gradient-to-br from-accent via-muted to-secondary", className)}>
    <Gamepad2Icon className="max-h-[35%] size-12 text-primary/40" aria-hidden="true" />
    {!src && <span className="text-xs text-muted-foreground">{tr("无游戏封面")}</span>}
    {src && <img key={src} src={src} alt={decorative ? "" : tr("{name}的游戏封面", { name })} loading="lazy" decoding="async" referrerPolicy="no-referrer" className={cn("absolute inset-0 size-full object-cover motion-safe:transition-[opacity,scale] motion-safe:duration-500 motion-safe:ease-out", loadedUrl === src ? "scale-100 opacity-100 motion-safe:group-hover/cover:scale-105" : "motion-safe:scale-105 motion-safe:opacity-0")} onLoad={() => setLoadedUrl(src)} onError={() => setFailedUrl(src)} />}
  </div>;
}
