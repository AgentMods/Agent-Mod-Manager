import { tr } from "@/lib/i18n";
import { useEffect, useRef, useState, type RefObject } from "react";
import { flushSync } from "react-dom";
import { ArrowUpIcon, CheckIcon, ChevronDownIcon, ChevronRightIcon, PlusIcon, ShieldAlertIcon, ShieldIcon, SquareIcon } from "lucide-react";
import { DropdownMenu as Menu, Popover, Slider } from "radix-ui";
import { Button } from "@/components/ui/button";
import { Field, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { AttachmentGroup } from "@/components/ui/attachment";
import { Spinner } from "@/components/ui/spinner";
import { AgentPromptPresetDialog } from "./AgentPromptPresetDialog";
import { promptPresets, renderPreset, type PromptPresetId } from "@/lib/agent-prompt-presets";
import { ComposerAttachment, useAttachmentConverterStart } from "./AgentAttachments";
import { ErrorState } from "./ui";
import type { DraftAttachment } from "@/lib/agent-attachments";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { InputGroup, InputGroupAddon, InputGroupTextarea } from "@/components/ui/input-group";
import { reasoningEffortLabel } from "@/lib/reasoning-effort";
import type { AgentAttachment, AgentModelSettings, Game, PermissionMode, ReasoningEffort } from "@/lib/types";
import { cn } from "@/lib/utils";

const popupPosition = { side: "top", align: "end", sideOffset: 8, collisionPadding: 12 } as const;
const permissionLabels: Record<PermissionMode | "", string> = {
  get ""() { return tr("会话设置"); },
  get request_approval() { return tr("逐次审批"); },
  get auto_approve() { return tr("游戏内自动批准"); },
  get full_trust() { return tr("完全信任"); },
};

interface AgentComposerProps {
  input: string;
  inputRef: RefObject<HTMLTextAreaElement | null>;
  onCancelEdit?: () => void;
  onInputChange: (value: string) => void;
  attachments: DraftAttachment[];
  updateAttachments: (apply: (current: DraftAttachment[]) => DraftAttachment[]) => void;
  games: Game[];
  libraryGameName?: string;
  onSend: (attachments: AgentAttachment[], newGameName?: string) => void;
  onCancel?: () => void;
  disabled: boolean;
  busy: boolean;
  permission: PermissionMode | "";
  inheritedPermission: PermissionMode | null | undefined;
  onPermissionChange: (value: PermissionMode | "") => void;
  model: string;
  models: AgentModelSettings[];
  onModelChange: (value: string) => void;
  reasoningEffort: ReasoningEffort | null;
  onReasoningChange: (value: ReasoningEffort | null) => void;
}

export function AgentComposer({
  input, inputRef, onCancelEdit, onInputChange, attachments, updateAttachments, games, libraryGameName, onSend, onCancel, disabled, busy, permission, inheritedPermission,
  onPermissionChange, model, models, onModelChange, reasoningEffort, onReasoningChange,
}: AgentComposerProps) {
  const [picker, setPicker] = useState<"model" | "reasoning" | null>(null);
  const [editedGameName, setEditedGameName] = useState<string | null>(null);
  useEffect(() => setEditedGameName(null), [libraryGameName]);
  const trimmedName = editedGameName?.trim();
  const newGameName = libraryGameName !== undefined && trimmedName !== libraryGameName.trim() ? trimmedName : undefined;
  const invalidGameName = newGameName !== undefined && (!newGameName || /\p{Cc}/u.test(newGameName));
  const filePicker = useRef<HTMLInputElement>(null);
  const converter = useAttachmentConverterStart();
  const needsSetup = attachments.some((item) => item.state === "error" && item.needsConsent);
  const controlsDisabled = disabled || busy;
  const ready = attachments.filter((item) => item.state === "ready");
  const canSend = !controlsDisabled && !invalidGameName && ready.length === attachments.length && (Boolean(input.trim()) || ready.length > 0);

  const [presetPicker, setPresetPicker] = useState<{ id: PromptPresetId; opener: HTMLButtonElement } | null>(null);
  const selectionAfterFill = useRef<[number, number] | undefined>(undefined);
  const canUsePresets = !controlsDisabled && !onCancelEdit && libraryGameName === undefined && !attachments.length;
  const showPresets = canUsePresets && !input;
  const install = renderPreset("install", {});
  const showGameHelper = canUsePresets && input === install.draft;
  const presets = showPresets || showGameHelper ? promptPresets() : [];
  const activePreset = presets.find(preset => preset.id === presetPicker?.id);

  function focusDraft(selection?: [number, number]) {
    const textarea = inputRef.current;
    if (!textarea) return;
    textarea.focus();
    const [start, end] = selection ?? [textarea.value.length, textarea.value.length];
    textarea.setSelectionRange(start, end);
  }

  function retryFailed(shouldRetry: (item: Extract<DraftAttachment, { state: "error" }>) => boolean) {
    updateAttachments((current) => current.map((item) => item.state === "error" && shouldRetry(item)
      ? { id: item.id, state: "processing", file: item.file } : item));
  }

  function startConverter() {
    const draftIds = new Set(attachments.map((item) => item.id));
    void converter.start(() => retryFailed((item) => item.needsConsent === true && draftIds.has(item.id)));
  }

  function addFiles(files: File[]) {
    if (controlsDisabled || !files.length) return;
    const pending = files.map((file) => ({ id: crypto.randomUUID(), file, state: "processing" as const }));
    updateAttachments((current) => [...current, ...pending]);
  }
  const selectedModel = models.find((entry) => entry.id === model);
  const levels = selectedModel?.reasoning_levels ?? [];
  const modelLabel = selectedModel?.display_name?.trim() || model || tr("未配置模型");
  const effectivePermission = permission || inheritedPermission;
  const permissionLabel = effectivePermission ? permissionLabels[effectivePermission] : inheritedPermission === null ? tr("权限未记录") : tr("会话设置 · 读取中");
  const elevated = effectivePermission === "full_trust";
  const PermissionIcon = elevated ? ShieldAlertIcon : ShieldIcon;

  return <><form onSubmit={(event) => {
    event.preventDefault();
    if (canSend) onSend(ready.map((item) => item.attachment), newGameName);
  }}>
    {showPresets && <div role="group" aria-label={tr("预设提示词")} className="mb-2 flex flex-wrap gap-1.5">
      {presets.map(({ id, label }) => <Button key={id} type="button" variant="outline" size="sm" aria-haspopup={id === "install" ? undefined : "dialog"}
        className="h-auto max-w-full rounded-full py-1.5 text-xs whitespace-normal"
        onClick={event => {
          if (id !== "install") { setPresetPicker({ id, opener: event.currentTarget }); return; }
          // Commit the controlled textarea value before selecting its game-name slot.
          flushSync(() => onInputChange(install.draft));
          focusDraft(install.firstUnfilled);
        }}>{label}</Button>)}
    </div>}
    {showGameHelper && <Button type="button" size="sm" variant="outline" aria-haspopup="dialog" className="mb-2 rounded-full"
      onClick={event => setPresetPicker({ id: "install", opener: event.currentTarget })}>{tr("使用模板")}</Button>}
    {libraryGameName !== undefined && <Field className="mb-3 gap-1.5" data-invalid={invalidGameName || undefined}>
      <div className="flex items-center gap-3">
        <FieldLabel className="shrink-0" htmlFor="adapter-game-name">{tr("游戏库显示名称")}</FieldLabel>
        <Input id="adapter-game-name" className="min-w-0 max-w-sm" value={editedGameName ?? libraryGameName}
          aria-invalid={invalidGameName} aria-describedby="adapter-game-name-hint" disabled={controlsDisabled}
          onChange={(event) => setEditedGameName(event.target.value)} />
      </div>
      {invalidGameName ? <FieldError id="adapter-game-name-hint">{tr("请输入游戏名称，且不含控制字符。")}</FieldError>
        : <FieldDescription id="adapter-game-name-hint">{tr("绑定适配包时会同步更新游戏库中的名称。")}</FieldDescription>}
    </Field>}
    {onCancelEdit && <div className="mb-1 flex items-center justify-between gap-2 px-2 text-xs text-muted-foreground">
      <span>{tr("正在编辑消息")}</span>
      <Button type="button" size="xs" variant="ghost" disabled={busy} onClick={onCancelEdit}>{tr("取消编辑")}</Button>
    </div>}
    <input ref={filePicker} type="file" aria-label={tr("选择附件")} hidden multiple disabled={controlsDisabled} onChange={(event) => {
      const files = Array.from(event.currentTarget.files ?? []);
      event.currentTarget.value = "";
      addFiles(files);
    }} />
    <InputGroup className="rounded-[22px] border-border shadow-sm bg-muted/55 dark:bg-secondary">
      {attachments.length > 0 && <InputGroupAddon align="block-start" className="min-w-0 flex-col items-stretch gap-1 px-3 pt-2 pb-0">
        <AttachmentGroup aria-label={tr("待发送附件")} aria-live="polite" className="max-h-[25dvh] flex-wrap gap-1.5 overflow-y-auto">
          {attachments.map((draft) => <ComposerAttachment key={draft.id} draft={draft} disabled={controlsDisabled}
            onRemove={() => updateAttachments((current) => current.filter((item) => item.id !== draft.id))}
            onRetry={() => retryFailed((item) => item.id === draft.id)} />)}
        </AttachmentGroup>
        {needsSetup && <>
          <Button type="button" size="xs" variant="outline" className="self-start" disabled={controlsDisabled || converter.starting}
            aria-busy={converter.starting} onClick={startConverter}>
            {converter.starting && <Spinner data-icon="inline-start" />}{tr("启动 MarkItDown MCP")}
          </Button>
          <ErrorState error={converter.error} />
        </>}
      </InputGroupAddon>}
      <InputGroupTextarea
        ref={inputRef}
        aria-label={tr("告诉 Agent 你想做什么")}
        placeholder={tr("和 Agent 一起工作")}
        value={input}
        className="max-h-[min(14rem,34dvh)] min-h-16 resize-none px-4 pt-3.5 pb-2 sm:min-h-20 sm:text-base"
        disabled={disabled}
        onChange={(event) => onInputChange(event.target.value)}
        onPaste={(event) => {
          const data = event.clipboardData;
          const files = data.files.length ? Array.from(data.files) : Array.from(data.items)
            .filter((item) => item.kind === "file").map((item) => item.getAsFile()).filter((file) => file !== null);
          if (!files.length) return;
          event.preventDefault();
          addFiles(files);
        }}
        onKeyDown={(event) => {
          if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
            event.preventDefault();
            if (canSend) event.currentTarget.form?.requestSubmit();
          }
        }}
      />
      <InputGroupAddon align="block-end" className="cursor-default flex-wrap justify-between gap-1.5 px-2.5 pb-2.5 pt-0">
        <div className="flex min-w-0 items-center gap-1">
          <Button type="button" aria-label={tr("添加附件")} title={tr("添加附件（图片也可直接粘贴）")} size="icon-sm" variant="ghost"
            disabled={controlsDisabled} onClick={() => filePicker.current?.click()}>
            <PlusIcon aria-hidden="true" />
          </Button>
          <DropdownMenu modal={false}>
            <DropdownMenuTrigger asChild>
              <Button type="button" size="sm" variant="ghost" aria-label={tr("本轮权限策略")}
                title={permission ? permissionLabels[permission] : permissionLabel}
                className={cn("gap-1.5 rounded-full px-2 text-xs has-[>svg]:px-2", elevated && "text-warning")}
                disabled={controlsDisabled}>
                <PermissionIcon aria-hidden="true" className="size-4" />
                <span>{permissionLabel}</span>
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent {...popupPosition} align="start" className="min-w-44 rounded-xl">
              <DropdownMenuRadioGroup value={permission} onValueChange={(next) => onPermissionChange(next as PermissionMode | "")}>
                {Object.entries(permissionLabels).map(([value, name]) => (
                  <DropdownMenuRadioItem key={value} className="min-h-8" value={value}>{name}</DropdownMenuRadioItem>
                ))}
              </DropdownMenuRadioGroup>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
        <div className="ml-auto flex max-w-full min-w-0 items-center gap-2">
          <div className="flex min-w-0 items-center rounded-full bg-background/80 dark:bg-background/50">
            <DropdownMenu modal={false} open={!controlsDisabled && picker === "model"}
              onOpenChange={(open) => setPicker((current) => open ? "model" : current === "model" ? null : current)}>
              <DropdownMenuTrigger asChild>
                <Button type="button" size="sm" variant="ghost" aria-label={tr("本轮模型")} title={modelLabel}
                  className="min-w-0 shrink rounded-full px-2.5 has-[>svg]:px-2.5"
                  disabled={controlsDisabled || !models.length}>
                  <span className="max-w-[min(30vw,13rem)] truncate">{modelLabel}</span>
                  {!levels.length && <ChevronDownIcon aria-hidden="true" className="size-3 opacity-60" />}
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent {...popupPosition} aria-label={tr("本轮模型")}
                className="max-h-[min(23rem,var(--radix-dropdown-menu-content-available-height))] w-[min(19rem,calc(100vw-24px))] rounded-2xl p-1.5 shadow-lg">
                <DropdownMenuRadioGroup value={model} onValueChange={onModelChange}>
                  {models.map(({ id, display_name }) => (
                    <Menu.RadioItem key={id} value={id} title={id}
                      className="flex min-h-8 cursor-default items-center gap-3 rounded-lg px-2.5 py-1.5 text-sm outline-none select-none data-highlighted:bg-muted data-[state=checked]:font-medium">
                      <span className="min-w-0 flex-1 wrap-anywhere">{display_name?.trim() || id}</span>
                      <Menu.ItemIndicator><CheckIcon aria-hidden="true" className="size-4 text-primary" /></Menu.ItemIndicator>
                    </Menu.RadioItem>
                  ))}
                </DropdownMenuRadioGroup>
              </DropdownMenuContent>
            </DropdownMenu>
            {levels.length > 0 && <ReasoningPicker modelLabel={modelLabel} levels={levels} value={reasoningEffort}
              disabled={controlsDisabled} open={!controlsDisabled && picker === "reasoning"}
              onOpenChange={(open) => setPicker((current) => open ? "reasoning" : current === "reasoning" ? null : current)}
              onOpenModel={() => setPicker("model")} onChange={onReasoningChange} />}
          </div>
          {onCancel ? <Button key="stop" type="button" aria-label={tr("停止生成")} title={tr("停止生成")} variant="secondary" size="icon"
            className="rounded-full bg-foreground text-background hover:bg-foreground/85 motion-safe:animate-pop-in" onClick={onCancel}>
            <SquareIcon aria-hidden="true" className="size-3 fill-current" />
          </Button> : <Button key="send" type="submit" aria-label={tr("发送消息")} title={tr("发送消息")} size="icon" className="rounded-full motion-safe:animate-pop-in" disabled={!canSend}>
            <ArrowUpIcon aria-hidden="true" />
          </Button>}
        </div>
      </InputGroupAddon>
    </InputGroup>
  </form>
    {activePreset && presetPicker && <AgentPromptPresetDialog preset={activePreset} games={games} onClose={() => setPresetPicker(null)}
      onFill={(text, selection) => {
        selectionAfterFill.current = selection;
        setPresetPicker(null);
        onInputChange(text);
      }}
      onRestoreFocus={filled => {
        if (!filled) { if (presetPicker.opener.isConnected) presetPicker.opener.focus(); return; }
        focusDraft(selectionAfterFill.current);
      }} />}
  </>;
}

function ReasoningPicker({ modelLabel, disabled, levels, value, open, onOpenChange, onOpenModel, onChange }: {
  modelLabel: string;
  disabled: boolean;
  levels: ReasoningEffort[];
  value: ReasoningEffort | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onOpenModel: () => void;
  onChange: (value: ReasoningEffort | null) => void;
}) {
  const openingModel = useRef(false);
  const selected = value ? levels.indexOf(value) + 1 : 0;
  const accessibleLevel = value ? reasoningEffortLabel(value) : tr("服务默认");
  const ultra = value === "ultra";
  const levelLabel = ultra ? "Ultra" : accessibleLevel.split("（")[0];

  return <Popover.Root open={open} onOpenChange={onOpenChange}>
    <Popover.Trigger asChild>
      <Button type="button" size="sm" variant="ghost" aria-label={tr("本轮思考等级")} title={tr("推理强度 · {accessibleLevel}", { accessibleLevel })} disabled={disabled}
        className="gap-1 rounded-full pl-1 pr-2.5 text-xs text-muted-foreground has-[>svg]:pl-1 has-[>svg]:pr-2.5">
        <span className="truncate">{levelLabel}</span>
        <ChevronDownIcon aria-hidden="true" className="size-3 opacity-70" />
      </Button>
    </Popover.Trigger>
    <Popover.Portal>
      <Popover.Content {...popupPosition} aria-label={tr("本轮思考等级")} onCloseAutoFocus={(event) => {
        if (openingModel.current) {
          event.preventDefault();
          openingModel.current = false;
        }
      }}
        className="z-50 w-[min(19rem,calc(100vw-24px))] rounded-2xl border bg-popover p-4 text-center text-popover-foreground shadow-lg">
        <p className={cn("text-base font-semibold", ultra ? "text-reasoning-ultra" : "text-primary")}>{levelLabel}</p>
        <Button type="button" variant="ghost" size="xs" onClick={() => {
          openingModel.current = true;
          onOpenModel();
        }}
          className="max-w-full gap-1 text-muted-foreground" title={modelLabel}>
          <span className="truncate">{modelLabel}</span><ChevronRightIcon aria-hidden="true" />
        </Button>
        <Slider.Root className="relative mt-2.5 flex h-8 w-full touch-none items-center select-none"
          value={[selected]} min={0} max={levels.length} step={1} onValueChange={([index]) => onChange(levels[index - 1] ?? null)}>
          <Slider.Track className="relative h-6 w-full rounded-full bg-secondary">
            <Slider.Range className={cn("absolute h-full rounded-full", ultra ? "agent-ultra-range" : "bg-primary")} />
          </Slider.Track>
          {!ultra && <div aria-hidden="true" className="pointer-events-none absolute inset-x-3 flex justify-between">
            {["default", ...levels].map((level) => <span key={level} className="size-1 rounded-full bg-popover-foreground/30" />)}
          </div>}
          <Slider.Thumb aria-label={tr("推理强度")} aria-valuetext={accessibleLevel}
            className="block size-7 rounded-full border bg-white shadow-sm outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-popover" />
        </Slider.Root>
      </Popover.Content>
    </Popover.Portal>
  </Popover.Root>;
}
