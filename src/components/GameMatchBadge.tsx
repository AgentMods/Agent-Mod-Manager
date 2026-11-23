import { CircleCheckIcon } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import type { GameCandidate } from "@/lib/types";

export function GameMatchBadge({ candidate, loading, cloudMatched, variant = "secondary" }: { loading?: boolean; cloudMatched?: boolean; candidate: GameCandidate; variant?: "secondary" | "outline" }) {
  if (candidate.registeredGameId) return candidate.suggestedAdapterSlug
    ? <Badge variant="success"><CircleCheckIcon aria-hidden="true" />已适配</Badge>
    : <Badge variant={variant}>未关联适配包</Badge>;
  if (loading) return <Badge variant={variant}>正在匹配…</Badge>;
  if (cloudMatched) return <Badge variant="success">云端已匹配</Badge>;
  if (!candidate.suggestedAdapterSlug) {
    // The backend withholds an otherwise strong suggestion when the best matches are too close.
    const needsChoice = candidate.status === "candidate" && candidate.confidence >= 0.8;
    return <Badge variant={variant}>{needsChoice ? "待确认" : "未匹配"}</Badge>;
  }
  if (candidate.confidence === 1) {
    return candidate.status === "high_confidence"
      ? <Badge variant="success"><CircleCheckIcon aria-hidden="true" />已适配</Badge>
      : <Badge variant={variant}>待确认</Badge>;
  }
  return <Badge variant={variant}>{Math.min(99, Math.round(candidate.confidence * 100))}% 匹配</Badge>;
}
