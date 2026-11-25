import { useId, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderOpenIcon } from "lucide-react";
import { renderPreset, type PromptFieldId, type PromptPreset } from "@/lib/agent-prompt-presets";
import { tr } from "@/lib/i18n";
import type { Game } from "@/lib/types";
import { Button } from "./ui/button";
import { Field, FieldGroup, FieldLabel } from "./ui/field";
import { Input } from "./ui/input";
import { ErrorState, Modal } from "./ui";

type Props = {
  preset: PromptPreset;
  games: Game[];
  onClose: () => void;
  onFill: (text: string, selection?: [number, number]) => void;
  onRestoreFocus: (filled: boolean) => void;
};

export function AgentPromptPresetDialog({ preset, games, onClose, onFill, onRestoreFocus }: Props) {
  const prefix = useId();
  const [values, setValues] = useState<Partial<Record<PromptFieldId, string>>>({});
  const [browsing, setBrowsing] = useState(false);
  const [error, setError] = useState<unknown>();
  const filled = useRef(false);
  const complete = preset.fields.every(({ id }) => values[id]?.trim());

  function update(id: PromptFieldId, value: string) {
    setValues(current => ({ ...current, [id]: value }));
  }

  async function browse(id: PromptFieldId) {
    setError(undefined);
    setBrowsing(true);
    try {
      const path = await open({ directory: true, multiple: false, title: tr("选择文件夹") });
      if (typeof path === "string") update(id, path);
    } catch (failure) { setError(failure); }
    finally { setBrowsing(false); }
  }

  function fill() {
    const { draft, firstUnfilled } = renderPreset(preset.id, values);
    filled.current = true;
    onFill(draft, firstUnfilled);
  }

  return <Modal title={tr("使用模板")}
    className="sm:max-w-md" onClose={onClose}
    onCloseAutoFocus={event => { event.preventDefault(); onRestoreFocus(filled.current); }}
    actions={<>
      <Button type="button" variant="ghost" disabled={browsing} onClick={fill}>{tr("直接编辑提示词")}</Button>
      <Button form={prefix} type="submit" disabled={browsing || !complete}>{tr("填入草稿")}</Button>
    </>}>
    <form id={prefix} onSubmit={event => { event.preventDefault(); if (!browsing && complete) fill(); }}>
      <FieldGroup className="gap-4">
        {preset.fields.map(({ id, label }) => {
          const directory = id !== "gameName";
          const inputId = `${prefix}-${id}`;
          return <Field key={id} className="gap-1.5">
            <FieldLabel htmlFor={inputId}>{label}</FieldLabel>
            <div className="flex min-w-0 items-center gap-2">
              <Input id={inputId} className="min-w-0 flex-1" required disabled={browsing} autoComplete="off"
                placeholder={id === "gameName" ? tr("选择已有游戏或输入名称") : undefined}
                list={directory ? undefined : `${inputId}-options`}
                value={values[id] ?? ""} onChange={event => update(id, event.target.value)} />
              {!directory && <datalist id={`${inputId}-options`}>
                {[...new Set(games.map(game => game.name))].map(name => <option key={name} value={name} />)}
              </datalist>}
              {directory && <Button type="button" variant="outline" disabled={browsing} onClick={() => void browse(id)}>
                <FolderOpenIcon aria-hidden="true" />{tr("选择文件夹")}
              </Button>}
            </div>
          </Field>;
        })}
      </FieldGroup>
    </form>
    <ErrorState error={error} />
  </Modal >;
}
