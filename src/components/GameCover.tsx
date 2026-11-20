import { useState } from "react";
import { Gamepad2Icon } from "lucide-react";

export function GameCover({ name, url }: { name: string; url?: string | null }) {
  const [failedUrl, setFailedUrl] = useState<string | null>(null);
  return <div className="relative flex aspect-[16/8] w-full items-center justify-center overflow-hidden bg-gradient-to-br from-accent via-muted to-secondary">
    <Gamepad2Icon className="size-12 text-primary/40" aria-hidden="true" />
    <span aria-hidden="true" className="absolute bottom-2 left-4 text-3xl font-semibold tracking-tight text-foreground/60">{Array.from(name).slice(0, 2).join("")}</span>
    {url && url !== failedUrl && <img key={url} src={url} alt="" loading="lazy" decoding="async" referrerPolicy="no-referrer" className="absolute inset-0 size-full object-cover" onError={() => setFailedUrl(url)} />}
  </div>;
}
