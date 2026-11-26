import { tr } from "@/lib/i18n";
import { useId, useMemo, useRef, useState, type FormEvent } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ChevronDownIcon, ChevronLeftIcon, ChevronRightIcon, DownloadIcon, ExternalLinkIcon, GripVerticalIcon, PlusIcon, SaveIcon, ShieldCheckIcon, Trash2Icon, TriangleAlertIcon } from "lucide-react";
import { api, backendAvailable, errorText, useBackendQuery } from "@/lib/api";
import { REASONING_LEVELS, reasoningEffortLabel } from "../lib/reasoning-effort";
import { useUI } from "../lib/store";
import { cn } from "@/lib/utils";
import type { AgentModelSettings, AppSettings, ReasoningEffort } from "../lib/types";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { DropdownMenu, DropdownMenuCheckboxItem, DropdownMenuContent, DropdownMenuGroup, DropdownMenuLabel, DropdownMenuTrigger } from "@/components/ui/dropdown-menu";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Field, FieldDescription, FieldGroup, FieldLabel, FieldLegend, FieldSet } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Spinner } from "@/components/ui/spinner";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";

const KEY_URL = "https://one.gloscai.com/keys";
const MODEL_CANDIDATES_PER_PAGE = 100;
const modelColumnHead = "h-7 px-1 text-xs text-muted-foreground";
const modelTableCell = "min-w-0 p-0 @min-[37rem]:px-1 @min-[37rem]:py-0.5";
const modelInput = "h-8 px-2 text-sm";
const compactFieldLabel = "mb-1 block text-xs text-muted-foreground @min-[37rem]:hidden";
const modelMetadataQuery = "get_agent_model_metadata";
type ModelDraft = Omit<AgentModelSettings, "reasoning_levels"> & {
  rowKey: number;
  lookupId: string;
  focusOnAdd?: boolean;
  reasoning_levels: ReasoningEffort[] | null;
};
type Draft = Omit<AppSettings, "models" | "model"> & {models: ModelDraft[]; defaultModelKey: number | undefined};
type ModelMetadata = {id: string; reasoningLevels: ReasoningEffort[]};
type Props = {
  onSaved?: () => void;
  onSavingChange?: (saving: boolean) => void;
  navigation?: {onBack: () => void; onSkip: () => void};
};

function initialDraft({model: defaultId, models: savedModels = [], ...settings}: AppSettings): Draft {
  const legacyId = defaultId.trim();
  const configured = legacyId && !savedModels.some((model) => model.id === legacyId)
    ? [{display_name: legacyId, id: legacyId, reasoning_levels: []}, ...savedModels]
    : savedModels;
  const models = configured.map((model, rowKey) => ({
    ...model,
    display_name: model.display_name || model.id,
    // Loaded rows and newly added rows use separate key ranges.
    rowKey: -rowKey - 1,
    lookupId: model.id,
  }));
  return {...settings, models, defaultModelKey: models.find((model) => model.id === legacyId)?.rowKey ?? models[0]?.rowKey};
}

function validModelText(value: string) {
  return !!value && new TextEncoder().encode(value).length <= 256 && !/\p{Cc}/u.test(value);
}

function summarizeLevels(enabled: ReasoningEffort[]) {
  const groups: ReasoningEffort[][] = [];
  for (const level of REASONING_LEVELS) {
    if (!enabled.includes(level)) continue;
    const group = groups.at(-1);
    if (group && REASONING_LEVELS.indexOf(group.at(-1)!) + 1 === REASONING_LEVELS.indexOf(level)) group.push(level);
    else groups.push([level]);
  }
  return groups.map((group) => group.length >= 3 ? `${group[0]} → ${group.at(-1)}` : group.join(", ")).join(", ") || tr("服务默认");
}

export function AgentConfiguration({onSaved, onSavingChange, navigation}: Props) {
  const settings = useBackendQuery<AppSettings>(["settings"], "get_settings");
  const queryClient = useQueryClient();
  const notify = useUI((state) => state.notify);
  const [edited, setEdited] = useState<Draft>();
  const [apiKey, setApiKey] = useState("");
  const nextRowKey = useRef(0);
  const [serviceModels, setServiceModels] = useState<string[] | null>(null);
  const [selection, setSelection] = useState<string[]>([]);
  const [filter, setFilter] = useState("");
  const [candidatePage, setCandidatePage] = useState(0);
  const candidateList = useRef<HTMLDivElement>(null);
  const [listing, setListing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [draggingRow, setDraggingRow] = useState<number>();
  const [listError, setListError] = useState<string>();
  const [error, setError] = useState<string>();
  const id = useId();

  const saved = useMemo(() => settings.data && initialDraft(settings.data), [settings.data]);
  const draft = edited ?? saved;
  const baseUrl = draft?.base_url.trim() ?? "";
  const modelIds = useMemo(() => draft?.models.map((model) => model.id.trim()) ?? [], [draft?.models]);
  const configuredIds = useMemo(() => new Set(modelIds), [modelIds]);
  const lookupIds = [...new Set(draft?.models.map((model) => model.lookupId).filter(validModelText))].sort();
  const metadata = useBackendQuery<ModelMetadata[]>(["agent-model-metadata", baseUrl, lookupIds],
    modelMetadataQuery, {baseUrl, modelIds: lookupIds}, !!baseUrl && lookupIds.length > 0);
  const metadataById = new Map(metadata.data?.map((model) => [model.id, model]));
  const disabled = !draft || saving;
  const available = useMemo(() => {
    const query = filter.trim().toLowerCase();
    return serviceModels?.filter((modelId) => !configuredIds.has(modelId) && (!query || modelId.toLowerCase().includes(query)));
  }, [serviceModels, configuredIds, filter]);
  const candidatePages = Math.max(1, Math.ceil((available?.length ?? 0) / MODEL_CANDIDATES_PER_PAGE));
  const currentPage = Math.min(candidatePage, candidatePages - 1);
  const pageStart = currentPage * MODEL_CANDIDATES_PER_PAGE;
  const selectedIds = new Set(selection.filter((modelId) => !configuredIds.has(modelId)));
  const seenIds = new Set<string>();
  const duplicateIds = new Set<string>();
  for (const modelId of modelIds) {
    if (!modelId) continue;
    if (seenIds.has(modelId)) duplicateIds.add(modelId);
    seenIds.add(modelId);
  }
  const invalidModelIds = duplicateIds.size > 0 || modelIds.some((modelId) => !validModelText(modelId));
  const invalidNames = draft?.models.some((model) => !!model.display_name.trim() && !validModelText(model.display_name.trim()));
  const waitingForDefaults = metadata.isFetching && draft?.models.some((model) =>
    model.id.trim() === model.lookupId && model.reasoning_levels === null);
  const defaultModel = draft?.models.find((model) => model.rowKey === draft.defaultModelKey);
  const canSubmit = !(disabled || listing || invalidModelIds || invalidNames || selectedIds.size || (navigation && !defaultModel));

  function resetModelList() {
    setServiceModels(null);
    setSelection([]);
    setFilter("");
    setCandidatePage(0);
    setListError(undefined);
  }

  function showCandidatePage(page: number) {
    setCandidatePage(page);
    candidateList.current?.closest("[data-slot=scroll-area-viewport]")?.scrollTo({top: 0});
  }

  async function listModels() {
    setListing(true);
    resetModelList();
    try {
      setServiceModels(await api<string[]>("list_agent_models", {baseUrl, apiKey: apiKey.trim() || null}));
    } catch (failure) {
      setListError(errorText(failure));
    } finally {
      setListing(false);
    }
  }

  function appendModels(requested: string[], focusOnAdd = false) {
    if (!draft) return;
    const additions = requested.map((id) => ({
      id, display_name: id, rowKey: nextRowKey.current++, lookupId: id, focusOnAdd,
      reasoning_levels: null,
    }));
    setEdited({...draft, defaultModelKey: draft.defaultModelKey ?? additions[0]?.rowKey, models: [...draft.models, ...additions]});
    if (!focusOnAdd) resetModelList();
    setError(undefined);
  }

  function updateModel(rowKey: number, change: Partial<ModelDraft>) {
    if (!draft) return;
    const models = draft.models.map((model) => model.rowKey === rowKey ? {...model, ...change} : model);
    setEdited({...draft, models});
  }

  function removeModel(rowKey: number) {
    if (!draft) return;
    const models = draft.models.filter((model) => model.rowKey !== rowKey);
    const defaultModelKey = draft.defaultModelKey === rowKey ? models[0]?.rowKey : draft.defaultModelKey;
    setEdited({...draft, models, defaultModelKey});
  }

  function moveModel(rowKey: number, to: number) {
    if (!draft || to < 0 || to >= draft.models.length) return;
    const from = draft.models.findIndex((model) => model.rowKey === rowKey);
    if (from < 0 || from === to) return;
    const models = [...draft.models];
    const [moved] = models.splice(from, 1);
    models.splice(to, 0, moved);
    setEdited({...draft, models});
  }

  function levelsFor(model: ModelDraft, defaults = metadataById) {
    return model.reasoning_levels ?? defaults.get(model.id.trim())?.reasoningLevels ?? [];
  }

  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!draft || !canSubmit) return;
    setSaving(true);
    onSavingChange?.(true);
    setError(undefined);
    try {
      const key = apiKey.trim();
      const {models: rows, defaultModelKey: _, ...settings} = draft;
      // A click on Save can also be the first blur of an edited request ID.
      // Resolve defaults for the IDs being saved, even when that lookup just started.
      let defaults = metadataById;
      if (rows.some((model) => model.reasoning_levels === null)) {
        const idsToResolve = [...modelIds].sort();
        const queryKey = ["agent-model-metadata", baseUrl, idsToResolve];
        if (queryClient.getQueryState(queryKey)?.status !== "error") {
          const matches = await queryClient.fetchQuery({
            queryKey, queryFn: () => api<ModelMetadata[]>(modelMetadataQuery, {baseUrl, modelIds: idsToResolve}),
            staleTime: Infinity, retry: false,
          }).catch(() => []);
          defaults = new Map(matches.map((model) => [model.id, model]));
        }
      }
      const models = rows.map((model) => ({
        display_name: model.display_name.trim() || model.id.trim(),
        id: model.id.trim(),
        reasoning_levels: levelsFor(model, defaults),
      }));
      const next: AppSettings = {base_url: baseUrl, models, model: defaultModel?.id.trim() ?? "",
        api_key_ref: key ? "model.api_key" : settings.api_key_ref,
        permission_mode: settings.permission_mode, onboarding_completed: settings.onboarding_completed};
      if (key) await api("set_secret", {key: "model.api_key", value: key});
      await api("set_settings", {settings: next});
      setApiKey("");
      queryClient.setQueryData(["settings"], next);
      setEdited(undefined);
      notify(navigation ? tr("Agent 配置已保存") : tr("模型设置已保存"));
      onSaved?.();
    } catch (failure) {
      const message = errorText(failure);
      const key = apiKey.trim();
      setError(key ? message.replaceAll(key, tr("[已隐藏]")) : message);
    } finally {
      setSaving(false);
      onSavingChange?.(false);
    }
  }

  return <form className={cn("flex flex-col", navigation ? "gap-4" : "gap-3")} onSubmit={(event) => {void save(event);}}>
    <FieldGroup className={cn("gap-3", navigation && "px-6")}>
      {(error || settings.error) && <Alert variant="destructive"><AlertDescription>{error || errorText(settings.error)}</AlertDescription></Alert>}
      <div className="grid gap-3 rounded-xl border bg-muted/25 p-3 transition-colors duration-200 hover:border-foreground/20 sm:grid-cols-2">
        <Field className="gap-1.5">
          <FieldLabel htmlFor={`${id}-url`} className="min-h-5.5 items-center">{tr("API 地址")}</FieldLabel>
          <Input id={`${id}-url`} type="url" value={draft?.base_url ?? ""} disabled={disabled || listing} required
            placeholder="https://api.example.com/v1" onChange={(event) => {
              if (draft) setEdited({...draft, base_url: event.target.value});
              resetModelList();
            }}/>
        </Field>
        <Field className="gap-1.5">
          <div className="flex min-h-5.5 flex-wrap items-center justify-between gap-2">
            <FieldLabel htmlFor={`${id}-key`}>{tr("API 密钥")}</FieldLabel>
            <a className="inline-flex items-center gap-1 rounded-full border bg-background/80 px-2.5 py-0.5 text-xs font-medium text-foreground transition-colors hover:bg-muted" href={KEY_URL} target="_blank" rel="noreferrer" onClick={(event) => {
              if (!backendAvailable) return;
              event.preventDefault();
              void openUrl(KEY_URL).catch((failure) => setError(errorText(failure)));
            }}>{tr("没有AI？从这里获取")} <ExternalLinkIcon className="size-3" aria-hidden="true"/></a>
          </div>
          <Input id={`${id}-key`} type="password" autoComplete="new-password" value={apiKey} disabled={disabled || listing}
            placeholder={draft?.api_key_ref ? tr("已加密保存，留空保留") : tr("输入服务商提供的 API Key")}
            onChange={(event) => {setApiKey(event.target.value); resetModelList();}}/>
        </Field>
      </div>

      {draft && <section aria-labelledby={`${id}-models`} className="@container flex min-w-0 flex-col gap-2.5 rounded-xl border bg-muted/25 p-3 transition-colors duration-200 hover:border-foreground/20">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex min-w-0 flex-wrap items-baseline gap-x-2.5 gap-y-0.5">
            <h3 id={`${id}-models`} className="text-sm font-semibold">{tr("模型列表")}</h3>
            <FieldDescription role="status" className={cn("text-xs", metadata.isError && "text-warning")}>
              {waitingForDefaults ? tr("正在读取模型元数据…") : metadata.isError ? tr("暂时无法读取 BaseLLM 元数据") : tr("保存后应用到新的 Agent 请求。")}
            </FieldDescription>
          </div>
          <div className="flex flex-wrap items-center gap-1.5">
            <Button type="button" variant="outline" size="sm" className="bg-background/80 hover:bg-muted/60" disabled={disabled || listing || !baseUrl} onClick={() => {void listModels();}}>
              {listing ? <Spinner data-icon="inline-start"/> : <DownloadIcon data-icon="inline-start"/>}
              {listing ? tr("获取中…") : tr("获取模型列表")}
            </Button>
            <Button type="button" variant="outline" size="sm" className="bg-background/80 hover:bg-muted/60" disabled={disabled} onClick={() => appendModels([""], true)}>
              <PlusIcon data-icon="inline-start"/>{tr("手动添加")}
            </Button>
          </div>
        </div>

        {listError && <p className="text-sm text-warning" role="status">{tr("{listError}。也可以手动添加模型 ID。", { listError })}</p>}
        {available && <FieldSet disabled={disabled} className="gap-2.5 rounded-xl border bg-card/75 p-3">
          <FieldLegend variant="label" className="mb-0">{tr("服务商模型（可多选）")}</FieldLegend>
          <Label htmlFor={`${id}-filter`} className="sr-only">{tr("搜索服务商模型")}</Label>
          <Input id={`${id}-filter`} type="search" value={filter} onChange={(event) => {
            setFilter(event.target.value);
            showCandidatePage(0);
          }} placeholder={tr("搜索模型 ID")}/>
          <ScrollArea className="min-w-0 rounded-lg border bg-background/60 [&_[data-slot=scroll-area-viewport]]:max-h-40 [&_[data-slot=scroll-area-viewport]>div]:block!">
            <div ref={candidateList} className="flex flex-col gap-2 p-2.5">
              {available.slice(pageStart, pageStart + MODEL_CANDIDATES_PER_PAGE).map((name) => <Label key={name} className="font-normal break-all">
                <Checkbox checked={selectedIds.has(name)} onCheckedChange={(checked) => setSelection((current) => checked === true ? [...current, name] : current.filter((item) => item !== name))}/>
                <span>{name}</span>
              </Label>)}
              {!available.length && <p className="text-sm text-muted-foreground">{tr("没有可添加的匹配模型。")}</p>}
            </div>
          </ScrollArea>
          {available.length > MODEL_CANDIDATES_PER_PAGE && <div className="flex flex-wrap items-center gap-2">
            <Button type="button" variant="outline" size="sm" disabled={currentPage === 0} onClick={() => showCandidatePage(currentPage - 1)}>{tr("上一页模型")}</Button>
            <span role="status" className="text-xs text-muted-foreground tabular-nums">{tr("第 {value1} / {candidatePages} 页，共 {length} 个模型", { value1: currentPage + 1, candidatePages, length: available.length })}</span>
            <Button type="button" variant="outline" size="sm" disabled={currentPage + 1 === candidatePages} onClick={() => showCandidatePage(currentPage + 1)}>{tr("下一页模型")}</Button>
          </div>}
          <div className="flex flex-wrap gap-2">
            <Button type="button" size="sm" variant="outline" disabled={!selectedIds.size} onClick={() => appendModels([...selectedIds])}>{tr("添加所选（{size}）", { size: selectedIds.size })}</Button>
            <Button type="button" size="sm" variant="ghost" onClick={resetModelList}>{tr("收起")}</Button>
          </div>
        </FieldSet>}

        {draft.models.length ? <div className="min-w-0 overflow-hidden rounded-xl border bg-card/75">
          <Table aria-label={tr("模型列表")} role="table" className="block @min-[37rem]:table @min-[37rem]:table-fixed">
            <TableHeader role="rowgroup" className="sr-only @min-[37rem]:not-sr-only @min-[37rem]:table-header-group">
              <TableRow role="row" className="bg-muted/45 hover:bg-muted/45">
                <TableHead role="columnheader" scope="col" className="h-7 w-9" aria-label={tr("排序")}/>
                <TableHead role="columnheader" scope="col" className={modelColumnHead}>{tr("菜单显示名")}</TableHead>
                <TableHead role="columnheader" scope="col" className={modelColumnHead}>{tr("实际请求模型")}</TableHead>
                <TableHead role="columnheader" scope="col" className={cn(modelColumnHead, "w-[24%]")}>{tr("思考等级")}</TableHead>
                <TableHead role="columnheader" scope="col" className="h-7 w-9" aria-label={tr("删除")}/>
              </TableRow>
            </TableHeader>
            <TableBody role="rowgroup" className="block @min-[37rem]:table-row-group">
              {draft.models.map((model, index) => {
                const {rowKey} = model;
                const requestId = model.id.trim();
                const displayName = model.display_name.trim();
                const fieldName = requestId || tr("新模型");
                const rowName = displayName || fieldName;
                const enabledLevels = levelsFor(model);
                return <TableRow key={rowKey} role="row" data-state={draggingRow === rowKey ? "selected" : undefined}
                  className="grid grid-cols-2 gap-x-2 gap-y-1.5 p-2 transition-colors hover:bg-muted/35 @min-[37rem]:table-row @min-[37rem]:p-0"
                  onDragOver={(event) => {
                    if (draggingRow === undefined || disabled) return;
                    event.preventDefault();
                    event.dataTransfer.dropEffect = "move";
                  }}
                  onDrop={(event) => {
                    if (draggingRow === undefined || disabled) return;
                    event.preventDefault();
                    moveModel(draggingRow, index);
                    setDraggingRow(undefined);
                  }}>
                  <TableCell role="cell" className="min-w-0 p-0 @min-[37rem]:px-1 @min-[37rem]:py-0.5">
                    <Button type="button" variant="ghost" size="icon-sm" aria-label={tr("移动 {rowName}", { rowName })}
                      title={tr("拖动排序，或按 ↑ / ↓ 移动")} aria-keyshortcuts="ArrowUp ArrowDown"
                      disabled={disabled || draft.models.length < 2} draggable={!disabled && draft.models.length > 1}
                      className="cursor-grab active:cursor-grabbing"
                      onDragStart={(event) => {
                        event.dataTransfer.effectAllowed = "move";
                        event.dataTransfer.setData("text/plain", String(rowKey));
                        setDraggingRow(rowKey);
                      }}
                      onDragEnd={() => setDraggingRow(undefined)}
                      onKeyDown={(event) => {
                        if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
                        event.preventDefault();
                        const handle = event.currentTarget;
                        moveModel(rowKey, index + (event.key === "ArrowUp" ? -1 : 1));
                        requestAnimationFrame(() => {
                          if (document.activeElement === document.body) handle.focus();
                        });
                      }}>
                      <GripVerticalIcon aria-hidden="true"/>
                    </Button>
                  </TableCell>
                  <TableCell role="cell" className={modelTableCell}>
                    <span className={compactFieldLabel} aria-hidden="true">{tr("菜单显示名")}</span>
                    <Input aria-label={tr("{fieldName} 菜单显示名", { fieldName })} className={modelInput} value={model.display_name}
                      placeholder={requestId || tr("可留空")} disabled={disabled}
                      aria-invalid={!!displayName && !validModelText(displayName)}
                      onChange={(event) => updateModel(rowKey, {display_name: event.target.value})}/>
                  </TableCell>
                  <TableCell role="cell" className={modelTableCell}>
                    <span className={compactFieldLabel} aria-hidden="true">{tr("实际请求模型")}</span>
                    <Input aria-label={tr("{fieldName} 实际请求模型", { fieldName })} className={modelInput} value={model.id}
                      autoFocus={model.focusOnAdd} autoComplete="off" spellCheck={false} placeholder={tr("输入模型 ID")}
                      required disabled={disabled} aria-invalid={duplicateIds.has(requestId) || (!!requestId && !validModelText(requestId))}
                      onChange={(event) => updateModel(rowKey, {id: event.target.value})}
                      onBlur={() => {
                        if (requestId !== model.lookupId || requestId !== model.id) updateModel(rowKey, {id: requestId, lookupId: requestId});
                      }}/>
                  </TableCell>
                  <TableCell role="cell" className={cn(modelTableCell, "col-span-2 @min-[37rem]:col-auto")}>
                    <span className={compactFieldLabel} aria-hidden="true">{tr("思考等级")}</span>
                    <DropdownMenu modal={false}>
                      <DropdownMenuTrigger asChild>
                        <Button type="button" variant="outline" size="sm" className="w-full justify-between gap-1.5 px-2 font-normal"
                          aria-label={tr("{fieldName} 思考等级", { fieldName })} disabled={disabled} title={enabledLevels.join(", ") || tr("服务默认")}>
                          <span className="min-w-0 truncate">{summarizeLevels(enabledLevels)}</span><ChevronDownIcon aria-hidden="true"/>
                        </Button>
                      </DropdownMenuTrigger>
                      <DropdownMenuContent align="end" className="min-w-40">
                        <DropdownMenuGroup>
                          <DropdownMenuLabel>{tr("启用的思考等级")}</DropdownMenuLabel>
                          {REASONING_LEVELS.map((level) => <DropdownMenuCheckboxItem key={level}
                            checked={enabledLevels.includes(level)} onSelect={(event) => event.preventDefault()}
                            onCheckedChange={(checked) => updateModel(rowKey, {reasoning_levels: REASONING_LEVELS.filter((option) =>
                              option === level ? checked : enabledLevels.includes(option))})}>
                            {reasoningEffortLabel(level)}
                          </DropdownMenuCheckboxItem>)}
                        </DropdownMenuGroup>
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </TableCell>
                  <TableCell role="cell" className="col-start-2 row-start-1 justify-self-end p-0 text-right @min-[37rem]:px-1 @min-[37rem]:py-0.5">
                    <Button type="button" variant="ghost" size="icon-sm" aria-label={tr("删除 {rowName}", { rowName })} disabled={disabled}
                      title={tr("删除模型")} onClick={() => removeModel(rowKey)}>
                      <Trash2Icon aria-hidden="true"/>
                    </Button>
                  </TableCell>
                </TableRow>;
              })}
            </TableBody>
          </Table>
        </div> : <Empty className="rounded-xl border bg-card/60 p-3.5 md:p-3.5">
          <EmptyHeader><EmptyTitle>{tr("暂无模型")}</EmptyTitle><EmptyDescription>{tr("从服务商获取模型列表，或手动添加。")}</EmptyDescription></EmptyHeader>
        </Empty>}

        {modelIds.some((modelId) => !!modelId && !validModelText(modelId)) || invalidNames
          ? <FieldDescription role="status" className="text-xs">{tr("模型名称最多 256 字节，且不能包含控制字符。")}</FieldDescription> : null}
        {duplicateIds.size > 0 && <FieldDescription role="status" className="text-xs">{tr("实际请求模型 ID 不能重复。")}</FieldDescription>}
      </section>}

      {draft && draft.models.length > 1 ? <div className="grid gap-3 rounded-xl border bg-muted/25 p-3 transition-colors duration-200 hover:border-foreground/20 sm:grid-cols-2">
        <Field className="gap-1.5">
          <FieldLabel htmlFor={`${id}-default`} className="min-h-5 items-center">{tr("默认模型")}</FieldLabel>
          <NativeSelect id={`${id}-default`} className="w-full" value={defaultModel?.id ?? ""} disabled={disabled}
            onChange={(event) => {
              const model = draft.models.find((candidate) => candidate.id === event.target.value);
              if (model) setEdited({...draft, defaultModelKey: model.rowKey});
            }}>
            {draft.models.map((model) => <NativeSelectOption key={model.rowKey} value={model.id} disabled={!model.id.trim()}>
              {model.display_name.trim() || model.id || tr("新模型")}
            </NativeSelectOption>)}
          </NativeSelect>
        </Field>
        <Field className="gap-1.5">
          <FieldLabel htmlFor={`${id}-permission`} className="min-h-5 items-center gap-1.5">
            <ShieldCheckIcon aria-hidden="true" className="size-4 text-muted-foreground"/>
            {tr("默认权限策略")}
          </FieldLabel>
          <NativeSelect id={`${id}-permission`} className="w-full" value={draft.permission_mode} disabled={disabled} onChange={(event) => {
            setEdited({...draft, permission_mode: event.target.value as AppSettings["permission_mode"]});
          }}>
            <NativeSelectOption value="request_approval">{tr("逐次审批（推荐）")}</NativeSelectOption>
            <NativeSelectOption value="auto_approve">{tr("管理中的游戏目录内自动批准")}</NativeSelectOption>
            <NativeSelectOption value="full_trust">{tr("完全信任")}</NativeSelectOption>
          </NativeSelect>
        </Field>
      </div> : <div className="rounded-xl border bg-muted/25 px-3 py-2.5 transition-colors duration-200 hover:border-foreground/20">
        <Field orientation="responsive" className="gap-2">
          <FieldLabel htmlFor={`${id}-permission`} className="flex items-center gap-2 whitespace-nowrap">
            <ShieldCheckIcon aria-hidden="true" className="size-4 shrink-0 text-muted-foreground"/>
            {tr("默认权限策略")}
          </FieldLabel>
          <NativeSelect id={`${id}-permission`} className="w-full @md/field-group:min-w-64" value={draft?.permission_mode ?? "request_approval"} disabled={disabled} onChange={(event) => {
            if (draft) setEdited({...draft, permission_mode: event.target.value as AppSettings["permission_mode"]});
          }}>
            <NativeSelectOption value="request_approval">{tr("逐次审批（推荐）")}</NativeSelectOption>
            <NativeSelectOption value="auto_approve">{tr("管理中的游戏目录内自动批准")}</NativeSelectOption>
            <NativeSelectOption value="full_trust">{tr("完全信任")}</NativeSelectOption>
          </NativeSelect>
        </Field>
      </div>}
      {draft?.permission_mode === "full_trust" && <Alert variant="warning"><TriangleAlertIcon/><AlertDescription>{tr("完全信任会跳过工具确认，包括外部服务写入与发布。第三方 stdio 进程使用当前用户权限。")}</AlertDescription></Alert>}
    </FieldGroup>

    <div className={navigation ? "flex flex-wrap items-center gap-2 border-t bg-muted/20 p-6" : "flex flex-wrap items-center justify-end gap-2"}>
      {navigation && <><Button variant="ghost" type="button" className="group/back" disabled={saving} onClick={navigation.onBack}>
        <ChevronLeftIcon data-icon="inline-start" className="motion-safe:transition-transform motion-safe:duration-200 group-hover/back:-translate-x-0.5"/>
        {tr("上一步")}
      </Button>
      <span className="flex-1"/>
      <Button type="button" variant="ghost" disabled={saving} onClick={navigation.onSkip}>{tr("跳过配置")}</Button></>}
      <Button type="submit" className="group/save shadow-xs motion-safe:transition-shadow hover:shadow-md" disabled={!canSubmit}>
        {saving ? <Spinner data-icon="inline-start"/> : !navigation && <SaveIcon data-icon="inline-start"/>}
        {saving ? tr("保存中…") : navigation ? tr("保存并继续") : tr("保存设置")}
        {navigation && !saving && <ChevronRightIcon data-icon="inline-end" className="motion-safe:transition-transform motion-safe:duration-200 group-hover/save:translate-x-0.5"/>}
      </Button>
    </div>
  </form>;
}
