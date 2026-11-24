import { tr } from "@/lib/i18n";
import type { ReasoningEffort } from "./types";

const effortLabels: Record<ReasoningEffort, string> = {
  get none() { return tr("关闭"); }, get minimal() { return tr("极低"); }, get low() { return tr("低"); }, get medium() { return tr("中"); }, get high() { return tr("高"); }, get xhigh() { return tr("极高"); }, get max() { return tr("最高"); }, get ultra() { return tr("超高"); },
};

export const REASONING_LEVELS = Object.keys(effortLabels) as ReasoningEffort[];

export function reasoningEffortLabel(level: ReasoningEffort) {
  return `${effortLabels[level]}（${level}）`;
}
