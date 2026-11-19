import { useId, useMemo, useState, type FormEvent } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ChevronLeftIcon, ChevronRightIcon, ExternalLinkIcon, PlusIcon, SaveIcon, TriangleAlertIcon, XIcon } from "lucide-react";
import { api, backendAvailable, errorText, useBackendQuery } from "@/lib/api";
import { reasoningEffortLabel } from "../lib/reasoning-effort";
import { useUI } from "../lib/store";
import type { AgentModelSettings, AppSettings, ReasoningEffort } from "../lib/types";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Field, FieldDescription, FieldGroup, FieldLabel, FieldLegend, FieldSet } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Item, ItemActions, ItemContent, ItemGroup, ItemTitle } from "@/components/ui/item";
import { Label } from "@/components/ui/label";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Spinner } from "@/components/ui/spinner";

const KEY_URL = "https://one.gloscai.com/keys";
const DEFAULT_CONTEXT = 32768;
const MAX_CONTEXT = 16777216;
const MAX_MODELS = 64;
type ModelDraft = Omit<AgentModelSettings, "context_budget" | "reasoning_levels"> & {
  context_budget: number | null;
  reasoning_levels: ReasoningEffort[] | null;
};
type Draft = Omit<AppSettings, "models"> & {models: ModelDraft[]};
type ModelMetadata = {id: string; contextBudget: number | null; reasoningLevels: ReasoningEffort[]};
type Props = {
  onSaved?: () => void;
  onSavingChange?: (saving: boolean) => void;
  navigation?: {onBack: () => void; onSkip: () => void};
};

function initialDraft(settings: AppSettings): Draft {
  const models = settings.models ?? [];
  const legacyModel = settings.model.trim();
  if (legacyModel && !models.some((model) => model.id === legacyModel)) {
    return {...settings, model: legacyModel, models: [
      {id: legacyModel, context_budget: settings.context_budget, reasoning_levels: []},
      ...models.slice(0, MAX_MODELS - 1),
    ]};
  }
  return {...settings, model: legacyModel || models[0]?.id || "", models};
}

export function AgentConfiguration({onSaved, onSavingChange, navigation}: Props) {
  const settings = useBackendQuery<AppSettings>(["settings"], "get_settings");
  const queryClient = useQueryClient();
  const notify = useUI((state) => state.notify);
  const [edited, setEdited] = useState<Draft>();
  const [apiKey, setApiKey] = useState("");
  const [manualId, setManualId] = useState("");
  const [serviceModels, setServiceModels] = useState<string[] | null>(null);
  const [selection, setSelection] = useState<string[]>([]);
  const [filter, setFilter] = useState("");
  const [listing, setListing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [listError, setListError] = useState<string>();
  const [error, setError] = useState<string>();
  const id = useId();

  const saved = useMemo(() => settings.data && initialDraft(settings.data), [settings.data]);
  const draft = edited ?? saved;
  const baseUrl = draft?.base_url.trim() ?? "";
  const modelIds = draft?.models.map((model) => model.id) ?? [];
  const metadata = useBackendQuery<ModelMetadata[]>(["agent-model-metadata", baseUrl, modelIds],
    "get_agent_model_metadata", {baseUrl, modelIds}, !!baseUrl && modelIds.length > 0);
  const metadataById = new Map(metadata.data?.map((model) => [model.id, model]));
  const disabled = !draft || saving;
  const available = serviceModels?.filter((modelId) => !modelIds.includes(modelId) && modelId.toLowerCase().includes(filter.trim().toLowerCase()));
  const validManualId = manualId.trim().length > 0 && !modelIds.includes(manualId.trim());
  const waitingForDefaults = metadata.isFetching && draft?.models.some((model) =>
    model.context_budget === null || model.reasoning_levels === null);

  function resetModelList() {
    setServiceModels(null);
    setSelection([]);
    setFilter("");
    setListError(undefined);
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

  function addModels(requested: string[]) {
    if (!draft) return;
    const additions = [...new Set(requested.map((name) => name.trim()))].filter((name) => name && !modelIds.includes(name));
    if (additions.some((name) => new TextEncoder().encode(name).length > 256 || /\p{Cc}/u.test(name))) {
      setError("模型 ID 必须在 256 字节以内，且不能包含控制字符。");
      return;
    }
    if (draft.models.length + additions.length > MAX_MODELS) {
      setError(`最多可添加 ${MAX_MODELS} 个模型。`);
      return;
    }
    if (!additions.length) return;
    const models = additions.map((modelId) => ({id: modelId, context_budget: null, reasoning_levels: null}));
    setEdited({...draft, model: draft.model || additions[0], models: [...draft.models, ...models]});
    setSelection((selected) => selected.filter((name) => !additions.includes(name)));
    if (additions.includes(manualId.trim())) setManualId("");
    setError(undefined);
  }

  function updateModel(modelId: string, change: Partial<ModelDraft>) {
    if (draft) setEdited({...draft, models: draft.models.map((model) => model.id === modelId ? {...model, ...change} : model)});
  }

  function removeModel(modelId: string) {
    if (!draft) return;
    const models = draft.models.filter((model) => model.id !== modelId);
    setEdited({...draft, models, model: draft.model === modelId ? models[0]?.id || "" : draft.model});
  }

  function contextFor(model: ModelDraft) {
    return model.context_budget ?? metadataById.get(model.id)?.contextBudget ?? DEFAULT_CONTEXT;
  }

  function levelsFor(model: ModelDraft) {
    return model.reasoning_levels ?? metadataById.get(model.id)?.reasoningLevels ?? [];
  }

  async function save(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!draft || saving || listing || waitingForDefaults || !draft.model || manualId.trim() || selection.length) return;
    setSaving(true);
    onSavingChange?.(true);
    setError(undefined);
    try {
      const key = apiKey.trim();
      const models = draft.models.map((model) => ({
        ...model, context_budget: contextFor(model), reasoning_levels: levelsFor(model),
      }));
      const next = {...draft, base_url: baseUrl, models,
        context_budget: models.find((model) => model.id === draft.model)?.context_budget ?? DEFAULT_CONTEXT,
        api_key_ref: key ? "model.api_key" : draft.api_key_ref};
      if (key) await api("set_secret", {key: "model.api_key", value: key});
      await api("set_settings", {settings: next});
      setApiKey("");
      queryClient.setQueryData(["settings"], next);
      setEdited(undefined);
      notify(navigation ? "Agent 配置已保存" : "模型设置已保存");
      onSaved?.();
    } catch (failure) {
      const message = errorText(failure);
      const key = apiKey.trim();
      setError(key ? message.replaceAll(key, "[已隐藏]") : message);
    } finally {
      setSaving(false);
      onSavingChange?.(false);
    }
  }

  const canSubmit = !(disabled || listing || waitingForDefaults || !draft?.model || !!manualId.trim() || !!selection.length);
  return <form className="flex flex-col gap-6" onSubmit={(event) => {void save(event);}}>
    <FieldGroup className={navigation ? "px-6" : undefined}>
      {(error || settings.error) && <Alert variant="destructive"><AlertDescription>{error || errorText(settings.error)}</AlertDescription></Alert>}
      <div className="grid gap-4 sm:grid-cols-2">
        <Field>
          <FieldLabel htmlFor={`${id}-url`}>API 地址</FieldLabel>
          <Input id={`${id}-url`} type="url" value={draft?.base_url ?? ""} disabled={disabled || listing} required
            placeholder="https://api.example.com/v1" onChange={(event) => {
              if (draft) setEdited({...draft, base_url: event.target.value});
              resetModelList();
            }}/>
        </Field>
        <Field>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <FieldLabel htmlFor={`${id}-key`}>API 密钥</FieldLabel>
            <a className="inline-flex items-center gap-1 text-xs text-primary underline-offset-4 hover:underline" href={KEY_URL} target="_blank" rel="noreferrer" onClick={(event) => {
              if (!backendAvailable) return;
              event.preventDefault();
              void openUrl(KEY_URL).catch((failure) => setError(errorText(failure)));
            }}>没有AI？从这里获取 <ExternalLinkIcon className="size-3" aria-hidden="true"/></a>
          </div>
          <Input id={`${id}-key`} type="password" autoComplete="new-password" value={apiKey} disabled={disabled || listing}
            placeholder={draft?.api_key_ref ? "已加密保存，留空保留" : "输入服务商提供的 API Key"}
            onChange={(event) => {setApiKey(event.target.value); resetModelList();}}/>
        </Field>
      </div>

      <FieldSet disabled={disabled}>
        <FieldLegend variant="label">添加模型</FieldLegend>
        <div className="flex flex-wrap items-center gap-2">
          <div className="min-w-48 flex-1">
            <Label htmlFor={`${id}-model`} className="sr-only">模型名称</Label>
            <Input id={`${id}-model`} value={manualId} placeholder="输入模型 ID" onChange={(event) => setManualId(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter" && !event.nativeEvent.isComposing) {
                  event.preventDefault();
                  addModels([manualId]);
                }
              }}/>
          </div>
          <Button type="button" variant="outline" disabled={!validManualId} onClick={() => addModels([manualId])}><PlusIcon data-icon="inline-start"/>添加模型</Button>
          <Button type="button" variant="outline" disabled={listing || !baseUrl} onClick={() => {void listModels();}}>{listing ? "获取中…" : "获取模型列表"}</Button>
        </div>
        {listError && <p className="text-sm text-warning" role="status">{listError}。也可以手动添加模型 ID。</p>}
        {available && <FieldSet className="gap-3 rounded-lg border p-4">
          <FieldLegend variant="label">服务商模型（可多选）</FieldLegend>
          <Label htmlFor={`${id}-filter`} className="sr-only">搜索服务商模型</Label>
          <Input id={`${id}-filter`} type="search" value={filter} onChange={(event) => setFilter(event.target.value)} placeholder="搜索模型 ID"/>
          <ScrollArea className="max-h-48 rounded-md border">
            <div className="flex flex-col gap-2 p-3">
              {available.slice(0, 100).map((name) => <Label key={name} className="font-normal break-all">
                <Checkbox checked={selection.includes(name)} onCheckedChange={(checked) => setSelection((current) => checked === true ? [...current, name] : current.filter((item) => item !== name))}/>
                <span>{name}</span>
              </Label>)}
              {!available.length && <p className="text-sm text-muted-foreground">没有可添加的匹配模型。</p>}
            </div>
          </ScrollArea>
          {available.length > 100 && <FieldDescription>请搜索以查看其他模型。</FieldDescription>}
          <div><Button type="button" size="sm" variant="outline" disabled={!selection.length} onClick={() => addModels(selection)}>添加所选（{selection.length}）</Button></div>
        </FieldSet>}
      </FieldSet>

      {!!draft?.models.length && <FieldSet disabled={disabled}>
        <FieldLegend variant="label">模型配置</FieldLegend>
        {draft.models.length > 1 && <Field>
          <FieldLabel htmlFor={`${id}-default`}>默认模型</FieldLabel>
          <NativeSelect id={`${id}-default`} className="w-full" value={draft.model} onChange={(event) => setEdited({...draft, model: event.target.value})}>
            {draft.models.map((model) => <NativeSelectOption key={model.id}>{model.id}</NativeSelectOption>)}
          </NativeSelect>
        </Field>}
        {metadata.isFetching && <p className="flex items-center gap-2 text-sm text-muted-foreground" role="status"><Spinner/>正在查找 BaseLLM 模型元数据…</p>}
        {metadata.isError && <p className="text-sm text-warning" role="status">暂时无法读取 BaseLLM 元数据，仍可手动填写上下文预算。</p>}
        <ItemGroup className="gap-3">
          {draft.models.map((model) => {
            const details = metadataById.get(model.id);
            const enabledLevels = levelsFor(model);
            const savedLevels = saved?.models.find((item) => item.id === model.id)?.reasoning_levels ?? [];
            const levels = [...new Set([...(details?.reasoningLevels ?? []), ...savedLevels, ...enabledLevels])];
            return <Item key={model.id} variant="outline" className="items-start">
              <ItemContent className="min-w-0 gap-4">
                <ItemTitle className="break-all">{model.id}</ItemTitle>
                <div className="grid gap-4 sm:grid-cols-2">
                  <Field>
                    <FieldLabel htmlFor={`${id}-${model.id}-ctx`}>上下文预算</FieldLabel>
                    <Input id={`${id}-${model.id}-ctx`} aria-label={`${model.id} 上下文预算`} type="number" min={1} max={MAX_CONTEXT} required
                      value={model.context_budget === 0 ? "" : contextFor(model)}
                      onChange={(event) => updateModel(model.id, {context_budget: Number(event.target.value)})}/>
                    {details?.contextBudget && <FieldDescription>BaseLLM 建议 {details.contextBudget.toLocaleString("zh-CN")} tokens</FieldDescription>}
                  </Field>
                  <FieldSet aria-label={`${model.id} 思考等级`}>
                    <FieldLegend variant="label">思考等级</FieldLegend>
                    {!!levels.length && <div className="flex flex-wrap gap-x-4 gap-y-2">
                      {levels.map((level) => <Label key={level} className="font-normal">
                        <Checkbox checked={enabledLevels.includes(level)} onCheckedChange={(next) => {
                          const checked = next === true;
                          updateModel(model.id, {reasoning_levels: levels.filter((option) =>
                            option === level ? checked : enabledLevels.includes(option))});
                        }}/>
                        <span>{reasoningEffortLabel(level)}</span>
                      </Label>)}
                    </div>}
                    {levels.length ? <FieldDescription>所选等级可在 Agent 对话中切换。</FieldDescription> :
                      !metadata.isFetching && <FieldDescription>该模型暂无可选的思考等级。</FieldDescription>}
                  </FieldSet>
                </div>
              </ItemContent>
              <ItemActions><Button type="button" variant="ghost" size="icon-sm" aria-label={`移除 ${model.id}`} onClick={() => removeModel(model.id)}><XIcon/></Button></ItemActions>
            </Item>;
          })}
        </ItemGroup>
      </FieldSet>}

      <Field>
        <FieldLabel htmlFor={`${id}-permission`}>默认权限策略</FieldLabel>
        <NativeSelect id={`${id}-permission`} className="w-full" value={draft?.permission_mode ?? "request_approval"} disabled={disabled} onChange={(event) => {
          if (draft) setEdited({...draft, permission_mode: event.target.value as AppSettings["permission_mode"]});
        }}>
          <NativeSelectOption value="request_approval">逐次审批（推荐）</NativeSelectOption>
          <NativeSelectOption value="auto_approve">已登记游戏目录内自动批准</NativeSelectOption>
          <NativeSelectOption value="full_trust">完全信任</NativeSelectOption>
        </NativeSelect>
      </Field>
      {draft?.permission_mode === "full_trust" && <Alert variant="warning"><TriangleAlertIcon/><AlertDescription>完全信任会跳过工具确认，包括外部服务写入与发布。第三方 stdio 进程使用当前用户权限。</AlertDescription></Alert>}
    </FieldGroup>

    <div className={navigation ? "flex flex-wrap items-center gap-2 border-t p-6" : "flex flex-wrap items-center gap-2"}>
      {navigation && <><Button variant="ghost" type="button" disabled={saving} onClick={navigation.onBack}><ChevronLeftIcon data-icon="inline-start"/>上一步</Button>
        <span className="flex-1"/><Button type="button" variant="ghost" disabled={saving} onClick={navigation.onSkip}>跳过配置</Button></>}
      <Button type="submit" disabled={!canSubmit}>
        {saving ? <Spinner data-icon="inline-start"/> : !navigation && <SaveIcon data-icon="inline-start"/>}{saving ? "保存中…" : navigation ? "保存并继续" : "保存设置"}{navigation && !saving && <ChevronRightIcon data-icon="inline-end"/>}
      </Button>
    </div>
  </form>;
}