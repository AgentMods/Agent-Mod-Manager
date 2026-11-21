import { CircleCheckIcon } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import type { GameCandidate } from "@/lib/types";

export function GameMatchBadge({ candidate, variant = "secondary" }: { candidate: GameCandidate; variant?: "secondary" | "outline" }) {
  if (candidate.confidence === 1) {
    return candidate.status === "high_confidence" && candidate.suggestedAdapterSlug
      ? <Badge variant="success"><CircleCheckIcon aria-hidden="true" />已适配</Badge>
      : <Badge variant={variant}>待确认</Badge>;
  }
  return <Badge variant={variant}>{Math.min(99, Math.round(candidate.confidence * 100))}% 匹配</Badge>;
}
