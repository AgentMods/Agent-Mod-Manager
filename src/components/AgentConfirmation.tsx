import { tr } from "@/lib/i18n";
import { useMemo, useRef, useState } from "react";
import { ShieldCheckIcon } from "lucide-react";
import { toast } from "sonner";
import { api, errorText } from "@/lib/api";
import { describeAgentTools, pendingToolApproval, replayConversation, useAgent } from "@/lib/agent-store";
import { displayApprovalPreview, isFilePlanApproval } from "@/lib/agent-path-presentation";
import { useModSwitch } from "@/lib/mod-switch-store";
import { useUI } from "@/lib/store";
import type { AgentEvent, FilePlan } from "@/lib/types";
import { Button } from "@/components/ui/button";
import { CodeBlock, CodeBlockContent } from "@/components/ui/code-block";
import { Item, ItemActions, ItemContent, ItemDescription, ItemMedia, ItemTitle } from "@/components/ui/item";
import { PlanPreview } from "./PlanDialog";
import { ErrorState } from "./ui";

import { PublicationListingPreview, type PublicationListing } from "./PublicationListing";

function publicationReview(value: unknown) {
  if (!value || typeof value !== "object" || !("preview" in value)) return null;
  const preview = value.preview;
  if (!preview || typeof preview !== "object" || !("listing" in preview) || !("target" in preview) || !("version" in preview)) return null;
  const listing = preview.listing;
  if (!listing || typeof listing !== "object" || !("title" in listing) || typeof listing.title !== "string" ||
      !("summary" in listing) || typeof listing.summary !== "string" || !("description" in listing) || typeof listing.description !== "string" ||
      ("coverUrl" in listing && listing.coverUrl != null && typeof listing.coverUrl !== "string") || !("tags" in listing) || !Array.isArray(listing.tags) ||
      !listing.tags.every(tag => typeof tag === "string") || typeof preview.version !== "string") return null;
  const target = preview.target;
  if (!target || typeof target !== "object" || !("kind" in target)) return null;
  let label = tr("首次发布");
  if (target.kind === "release" && "pluginId" in target && typeof target.pluginId === "number") {
    label = tr("更新插件 #{pluginId}", { pluginId: target.pluginId });
  } else if (target.kind !== "first") return null;
  const version = "remoteVersion" in preview && typeof preview.remoteVersion === "string" ? `${preview.remoteVersion} → ${preview.version}` : preview.version;
  return { listing: listing as PublicationListing, label, version, files: "files" in preview ? preview.files : undefined };
}

function findPlan(value: unknown): FilePlan | null {
  if (!value || typeof value !== "object") return null;
  const object = value as Record<string, unknown>;
  if (typeof object.id === "string" && Array.isArray(object.entries)) return object as unknown as FilePlan;
  return findPlan(object.plan) || findPlan(object.deployment);
}

export function AgentConfirmation({ request }: { request: AgentEvent }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const submitting = useRef(false);
  const { approvalId, title, preview } = request.payload;
  const publication = title === "publish_plugin" ? publicationReview(preview) : null;
  const plan = isFilePlanApproval(title) ? findPlan(preview) : null;
  const renameSummary = title === "set_game_adapter" && preview && typeof preview === "object" &&
    "gameName" in preview && typeof preview.gameName === "string" &&
    "targetGameName" in preview && typeof preview.targetGameName === "string" && preview.gameName !== preview.targetGameName
    ? tr("绑定后，游戏库中的「{gameName}」将改名为「{targetGameName}」。", { gameName: preview.gameName, targetGameName: preview.targetGameName }) : null;

  async function respond(approved: boolean) {
    if (submitting.current || (approved && publication?.listing.coverPng)) return;
    submitting.current = true;
    setBusy(true);
    setError(null);
    try {
      await api("respond_approval", { conversationId: request.conversationId, approvalId, response: { approved, answer: null } });
    } catch (failure) {
      submitting.current = false;
      setBusy(false);
      setError(failure);
      return;
    }
    // A successful response consumes the approval even if replay has to retry.
    void replayConversation(request.conversationId, { afterPending: true }).catch((failure) => toast.error(errorText(failure)));
  }

  const jsonPreview = <CodeBlock size="sm"><CodeBlockContent wrap className="max-h-64 break-all">
    {JSON.stringify(displayApprovalPreview(title, preview), null, 2)}
  </CodeBlockContent></CodeBlock>;

  return <Item role="group" aria-label={tr("操作审批")} aria-busy={busy} variant="outline" size="sm" className="w-full min-w-0 max-w-3xl bg-card">
    <ItemMedia variant="icon"><ShieldCheckIcon aria-hidden="true" /></ItemMedia>
    <ItemContent className="min-w-0">
      <ItemTitle>{publication ? tr("确认发布管理器插件") : tr("是否允许执行此操作？")}</ItemTitle>
      <ItemDescription className="line-clamp-none break-all font-mono text-xs">{publication ? tr("Mod客栈 · {label} · {version}", { label: publication.label, version: publication.version }) : String(title || tr("待确认操作"))}</ItemDescription>
    </ItemContent>
    <div className="flex min-w-0 basis-full flex-col gap-2">
      {renameSummary && <p className="wrap-anywhere text-sm">{renameSummary}</p>}
      {publication ? <>
        <PublicationListingPreview listing={publication.listing} files={publication.files} />
        {publication.listing.coverPng && <p className="text-sm text-muted-foreground">{tr("请拒绝此旧预览，重新发起发布以使用游戏封面。")}</p>}
        <details><summary className="cursor-pointer text-xs text-muted-foreground">{tr("查看完整发布请求")}</summary>{jsonPreview}</details>
      </> : plan ? <>
        <PlanPreview plan={plan} />
        <details><summary className="cursor-pointer text-xs text-muted-foreground">{tr("完整请求")}</summary><div className="mt-2">{jsonPreview}</div></details>
      </> : jsonPreview}
      <ErrorState error={error} />
    </div>
    <ItemActions className="ms-auto">
      <Button type="button" size="sm" variant="outline" disabled={busy} onClick={() => void respond(false)}>{tr("拒绝")}</Button>
      <Button type="button" size="sm" disabled={busy || !!publication?.listing.coverPng} onClick={() => void respond(true)}>{publication ? tr("发布") : tr("批准并执行")}</Button>
    </ItemActions>
  </Item>;
}

export function BackgroundApprovalLink({ visibleConversation }: { visibleConversation: string | null }) {
  const milestones = useAgent((state) => state.milestones);
  const openConversation = useUI((state) => state.openConversation);
  const switches = useModSwitch(state => state.pending);
  const pending = useMemo(() => {
    for (const [id, events] of Object.entries(milestones)) {
      if (id === visibleConversation) continue;
      const request = describeAgentTools(events).map(pendingToolApproval).find(Boolean);
      if (request) return { id, operation: Object.values(switches).find(entry =>
        entry.conversationId === id && entry.turnId === request.turnId) };
    }
  }, [milestones, visibleConversation, switches]);
  function showPending() {
    if (!pending) return;
    if (!pending.operation) { openConversation(pending.id); return; }
    // A Mod switch owns its own review on Game Details, not in the chat composer.
    const panel = document.getElementById(pending.operation.id);
    if (panel) { panel.focus({ preventScroll: true }); panel.scrollIntoView({ block: "nearest" }); }
    else useUI.getState().selectGame(pending.operation.gameId);
  }
  return pending ? <Button type="button" size="sm" variant="outline" onClick={showPending}>
    <ShieldCheckIcon aria-hidden="true" data-icon="inline-start" />{tr("查看待授权操作")}
  </Button> : null;
}
