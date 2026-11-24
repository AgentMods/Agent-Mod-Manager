import { tr } from "@/lib/i18n";
import { CircleCheckIcon } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import type { GameCandidate } from "@/lib/types";

export function GameMatchBadge({ candidate, loading, cloudMatched, variant = "secondary" }: { loading?: boolean; cloudMatched?: boolean; candidate: GameCandidate; variant?: "secondary" | "outline" }) {
  if (candidate.registeredGameId) return candidate.suggestedAdapterSlug
    ? <Badge variant="success"><CircleCheckIcon aria-hidden="true" />{tr("已适配")}</Badge>
    : <Badge variant={variant}>{tr("未关联适配包")}</Badge>;
  if (loading) return <Badge variant={variant}>{tr("正在匹配…")}</Badge>;
  if (cloudMatched) return <Badge variant="success">{tr("云端已匹配")}</Badge>;
  if (!candidate.suggestedAdapterSlug) {
    // The backend withholds an otherwise strong suggestion when the best matches are too close.
    const needsChoice = candidate.status === "candidate" && candidate.confidence >= 0.8;
    return <Badge variant={variant}>{needsChoice ? tr("待确认") : tr("未匹配")}</Badge>;
  }
  if (candidate.confidence === 1) {
    return candidate.status === "high_confidence"
      ? <Badge variant="success"><CircleCheckIcon aria-hidden="true" />{tr("已适配")}</Badge>
      : <Badge variant={variant}>{tr("待确认")}</Badge>;
  }
  return <Badge variant={variant}>{tr("{value1}% 匹配", { value1: Math.min(99, Math.round(candidate.confidence * 100)) })}</Badge>;
}
