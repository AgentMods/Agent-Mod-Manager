import { useState } from "react";
import { Gamepad2Icon } from "lucide-react";
import { cn } from "@/lib/utils";

export function GameCover({ name, url }: { name: string; url?: string | null }) {
  const [failedUrl, setFailedUrl] = useState<string | null>(null);
  const [loadedUrl, setLoadedUrl] = useState<string | null>(null);
  return <div className="group/cover relative flex aspect-[16/8] w-full items-center justify-center overflow-hidden bg-gradient-to-br from-accent via-muted to-secondary">
    <Gamepad2Icon className="size-12 text-primary/40" aria-hidden="true" />
    <span aria-hidden="true" className="absolute bottom-2 left-4 text-3xl font-semibold tracking-tight text-foreground/60">{Array.from(name).slice(0, 2).join("")}</span>
    {url && url !== failedUrl && <img key={url} src={url} alt="" loading="lazy" decoding="async" referrerPolicy="no-referrer" className={cn("absolute inset-0 size-full object-cover motion-safe:transition-[opacity,scale] motion-safe:duration-500 motion-safe:ease-out", loadedUrl === url ? "scale-100 opacity-100 motion-safe:group-hover/cover:scale-105" : "motion-safe:scale-105 motion-safe:opacity-0")} onLoad={() => setLoadedUrl(url)} onError={() => setFailedUrl(url)} />}
  </div>;
}
