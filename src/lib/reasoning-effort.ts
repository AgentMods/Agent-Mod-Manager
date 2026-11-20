import type { ReasoningEffort } from "./types";

const effortLabels: Record<ReasoningEffort, string> = {
  none: "关闭", minimal: "极低", low: "低", medium: "中", high: "高", xhigh: "极高", max: "最高", ultra: "超高",
};

export const REASONING_LEVELS = Object.keys(effortLabels) as ReasoningEffort[];

export function reasoningEffortLabel(level: ReasoningEffort) {
  return `${effortLabels[level]}（${level}）`;
}
