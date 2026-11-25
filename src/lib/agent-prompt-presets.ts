import { tr } from "@/lib/i18n";

export type PromptPresetId = "add" | "migrate" | "install";
export type PromptFieldId = "gameDirectory" | "oldModDirectory" | "gameName";

export type PromptPreset = { id: PromptPresetId; label: string; fields: { id: PromptFieldId; label: string }[] };
type PromptValues = Partial<Record<PromptFieldId, string>>;
type PresetDefinition = { label: () => string; fields: PromptFieldId[]; template: () => string };

const fieldLabels: Record<PromptFieldId, () => string> = {
  gameDirectory: () => tr("游戏目录"),
  oldModDirectory: () => tr("旧 Mod 存储目录"),
  gameName: () => tr("游戏名"),
};

const presets: Record<PromptPresetId, PresetDefinition> = {
  add: {
    label: () => tr("添加游戏"),
    fields: ["gameDirectory"],
    template: () => tr("添加游戏 {gameDirectory}"),
  },
  migrate: {
    label: () => tr("迁移旧管理器的 Mod"),
    fields: ["oldModDirectory"],
    template: () => tr("将 {oldModDirectory} 中的 Mod 从旧管理器迁移到新管理器。"),
  },
  install: {
    label: () => tr("查找并安装 Mod"),
    fields: ["gameName"],
    template: () => tr("帮我安装一些好玩的{gameName}mod"),
  },
};

export function promptPresets(): PromptPreset[] {
  return (Object.keys(presets) as PromptPresetId[]).map((id) => {
    const { label, fields } = presets[id];
    return { id, label: label(), fields: fields.map((id) => ({ id, label: fieldLabels[id]() })) };
  });
}

export function renderPreset(id: PromptPresetId, values: PromptValues) {
  const template = presets[id].template();
  let draft = "";
  let end = 0;
  let firstUnfilled: [number, number] | undefined;
  // Only visit slots in the localized template; inserted values are never scanned.
  for (const slot of template.matchAll(/\{(\w+)\}/g)) {
    const field = slot[1] as PromptFieldId;
    if (!Object.hasOwn(fieldLabels, field)) continue;
    draft += template.slice(end, slot.index);
    const value = values[field];
    if (value?.trim()) {
      draft += value.trim();
    } else {
      const placeholder = `[${fieldLabels[field]()}]`;
      firstUnfilled ??= [draft.length, draft.length + placeholder.length];
      draft += placeholder;
    }
    end = slot.index + slot[0].length;
  }
  return { draft: draft + template.slice(end), firstUnfilled };
}
