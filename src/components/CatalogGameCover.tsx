import { useEffect, useRef, useState } from "react";
import { useBackendQuery } from "@/lib/api";
import { cn } from "@/lib/utils";
import { GameCover } from "@/components/GameCover";
import { Skeleton } from "@/components/ui/skeleton";

export type ReleaseDisplay = { description: string; coverUrl: string | null };

// The public index contains metadata only. Fetch artwork for visible releases on demand.
export function CatalogGameCover({ releaseId, name, className }: { releaseId: string; name: string; className?: string }) {
  const container = useRef<HTMLDivElement>(null);
  const [visible, setVisible] = useState(false);
  useEffect(() => {
    if (!container.current) return;
    const observer = new IntersectionObserver(entries => {
      if (entries.some(entry => entry.isIntersecting)) {
        setVisible(true);
        observer.disconnect();
      }
    });
    observer.observe(container.current);
    return () => observer.disconnect();
  }, []);
  const display = useBackendQuery<ReleaseDisplay | null>(["manager-plugin-display", releaseId], "get_manager_plugin_display", {releaseId}, visible);
  return <div ref={container} className={cn("aspect-video w-full overflow-hidden rounded-lg", className)}>
    {visible && !display.isLoading
      ? <GameCover name={name} url={display.data?.coverUrl} className="size-full" />
      : <Skeleton className="size-full" />}
  </div>;
}
