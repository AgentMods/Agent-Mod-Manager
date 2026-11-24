import type { Locale } from "../lib/i18n";

type Values = Record<string, string | number | boolean | null | undefined>;
type InflectedLocale = Exclude<Locale, "zh-CN" | "ja" | "ko">;
type BinaryForms = string | readonly [one: string, other: string];
type RussianForms = readonly [one: string, few: string, many: string, other?: string];
type Forms = Record<Exclude<InflectedLocale, "ru">, BinaryForms> & { ru: RussianForms | string; ja?: string; ko?: string };
type Template = (locale: Exclude<Locale, "zh-CN">, values: Values) => string | undefined;

const rules = new Map<InflectedLocale, Intl.PluralRules>();

function select(locale: Exclude<Locale, "zh-CN">, count: Values[string], forms: Forms) {
  if (typeof count !== "number" || !Number.isFinite(count)) return;
  if (locale === "ja" || locale === "ko") return forms[locale];
  const variants = forms[locale];
  if (typeof variants === "string") return variants;
  let rule = rules.get(locale);
  if (!rule) {
    rule = new Intl.PluralRules(locale);
    rules.set(locale, rule);
  }
  const category = rule.select(count);
  if (locale !== "ru") return variants[category === "one" ? 0 : 1];
  // Fractional Russian counts use the genitive singular, usually shared with few.
  const [one, few, many, other = few] = variants;
  return category === "one" ? one : category === "few" ? few : category === "many" ? many : other;
}

function countBy(name: string, forms: Forms): Template {
  return (locale, values) => select(locale, values[name], forms);
}

// For sentences with independent counts, build from our own complete phrase templates.
// A frame runs before tr() interpolates the original placeholders and formats the numbers.
function countParts(frames: Record<Exclude<Locale, "zh-CN">, string>, parts: Record<string, Forms>): Template {
  return (locale, values) => {
    const resolved: Record<string, string> = {};
    for (const [name, forms] of Object.entries(parts)) {
      const part = select(locale, values[name], forms);
      if (part === undefined) return;
      resolved[name] = part;
    }
    return frames[locale].replace(/\[(\w+)\]/g, (token, name: string) => resolved[name] ?? token);
  };
}

// These are complete messages, including the invariant Japanese and Korean forms.
// They replace the old history adjective/suffix concatenation, not keys in the generated catalogs.
export const conversationCountSources = {
  active: "{length} 个活动对话",
  archived: "{length} 个已归档对话",
  group: "{length}个对话",
} as const;

// Keep the destructive scope and irreversible warning in each complete sentence.
export const trashPurgeSources = {
  all: "将永久删除回收站内全部 {length} 项（{size}）。此操作不可撤销。",
  selected: "将永久删除所选的 {length} 项（{size}）。此操作不可撤销。",
} as const;

const items = countBy("length", {
  en: ["{length} item", "{length} items"], de: ["{length} Eintrag", "{length} Einträge"],
  fr: ["{length} élément", "{length} éléments"], es: ["{length} elemento", "{length} elementos"],
  "pt-BR": ["{length} item", "{length} itens"], it: ["{length} elemento", "{length} elementi"],
  ru: ["{length} элемент", "{length} элемента", "{length} элементов"],
});

const selectedItems = countBy("length", {
  en: ["{length} item selected", "{length} items selected"], de: ["{length} Eintrag ausgewählt", "{length} Einträge ausgewählt"],
  fr: ["{length} élément sélectionné", "{length} éléments sélectionnés"], es: ["{length} elemento seleccionado", "{length} elementos seleccionados"],
  "pt-BR": ["{length} item selecionado", "{length} itens selecionados"], it: ["{length} elemento selezionato", "{length} elementi selezionati"],
  ru: ["Выбран {length} элемент", "Выбрано {length} элемента", "Выбрано {length} элементов", "Выбрано {length} элемента"],
});

const scanCounts = {
  directoriesChecked: {
    en: ["{directoriesChecked} directory", "{directoriesChecked} directories"], de: "{directoriesChecked} Ordner",
    fr: ["{directoriesChecked} dossier", "{directoriesChecked} dossiers"], es: ["{directoriesChecked} directorio", "{directoriesChecked} directorios"],
    "pt-BR": ["{directoriesChecked} diretório", "{directoriesChecked} diretórios"], it: "{directoriesChecked} directory",
    ru: ["{directoriesChecked} каталог", "{directoriesChecked} каталога", "{directoriesChecked} каталогов"], ja: "ディレクトリ {directoriesChecked} 件", ko: "디렉터리 {directoriesChecked}개",
  },
  entriesChecked: {
    en: ["{entriesChecked} entry", "{entriesChecked} entries"], de: ["{entriesChecked} Eintrag", "{entriesChecked} Einträge"],
    fr: ["{entriesChecked} entrée", "{entriesChecked} entrées"], es: ["{entriesChecked} entrada", "{entriesChecked} entradas"],
    "pt-BR": ["{entriesChecked} entrada", "{entriesChecked} entradas"], it: ["{entriesChecked} voce", "{entriesChecked} voci"],
    ru: ["{entriesChecked} элемент", "{entriesChecked} элемента", "{entriesChecked} элементов"], ja: "項目 {entriesChecked} 件", ko: "항목 {entriesChecked}개",
  },
} satisfies Record<string, Forms>;

// Full templates allow agreement and word order to vary as well as noun endings.
const templates: Record<string, Template> = {
  [trashPurgeSources.all]: countBy("length", {
    en: ["This will permanently delete everything in the Trash ({length} item, {size}). This action cannot be undone.", "This will permanently delete everything in the Trash ({length} items, {size}). This action cannot be undone."],
    de: ["Der gesamte Inhalt des Papierkorbs ({length} Eintrag, {size}) wird endgültig gelöscht. Dieser Vorgang kann nicht rückgängig gemacht werden.", "Der gesamte Inhalt des Papierkorbs ({length} Einträge, {size}) wird endgültig gelöscht. Dieser Vorgang kann nicht rückgängig gemacht werden."],
    fr: ["Tout le contenu de la corbeille ({length} élément, {size}) sera définitivement supprimé. Cette action est irréversible.", "Tout le contenu de la corbeille ({length} éléments, {size}) sera définitivement supprimé. Cette action est irréversible."],
    es: ["Se eliminará permanentemente todo el contenido de la papelera ({length} elemento, {size}). Esta acción no se puede deshacer.", "Se eliminará permanentemente todo el contenido de la papelera ({length} elementos, {size}). Esta acción no se puede deshacer."],
    "pt-BR": ["Todo o conteúdo da lixeira ({length} item, {size}) será excluído permanentemente. Essa ação não pode ser desfeita.", "Todo o conteúdo da lixeira ({length} itens, {size}) será excluído permanentemente. Essa ação não pode ser desfeita."],
    it: ["L’intero contenuto del cestino ({length} elemento, {size}) verrà eliminato definitivamente. Non è possibile annullare questa operazione.", "L’intero contenuto del cestino ({length} elementi, {size}) verrà eliminato definitivamente. Non è possibile annullare questa operazione."],
    ru: ["Всё содержимое корзины будет удалено безвозвратно ({length} элемент, {size}). Это действие нельзя отменить.", "Всё содержимое корзины будет удалено безвозвратно ({length} элемента, {size}). Это действие нельзя отменить.", "Всё содержимое корзины будет удалено безвозвратно ({length} элементов, {size}). Это действие нельзя отменить."],
    ja: "ごみ箱の中身をすべて（{length} 件、{size}）完全に削除します。この操作は取り消せません。",
    ko: "휴지통의 모든 항목({length}개, {size})을 영구적으로 삭제합니다. 이 작업은 취소할 수 없습니다.",
  }),
  [trashPurgeSources.selected]: countBy("length", {
    en: ["This will permanently delete {length} selected item ({size}). This action cannot be undone.", "This will permanently delete {length} selected items ({size}). This action cannot be undone."],
    de: ["{length} ausgewählter Eintrag ({size}) wird endgültig aus dem Papierkorb gelöscht. Dieser Vorgang kann nicht rückgängig gemacht werden.", "{length} ausgewählte Einträge ({size}) werden endgültig aus dem Papierkorb gelöscht. Dieser Vorgang kann nicht rückgängig gemacht werden."],
    fr: ["{length} élément sélectionné ({size}) sera définitivement supprimé de la corbeille. Cette action est irréversible.", "{length} éléments sélectionnés ({size}) seront définitivement supprimés de la corbeille. Cette action est irréversible."],
    es: ["Se eliminará permanentemente {length} elemento seleccionado de la papelera ({size}). Esta acción no se puede deshacer.", "Se eliminarán permanentemente {length} elementos seleccionados de la papelera ({size}). Esta acción no se puede deshacer."],
    "pt-BR": ["{length} item selecionado ({size}) será excluído permanentemente da lixeira. Essa ação não pode ser desfeita.", "{length} itens selecionados ({size}) serão excluídos permanentemente da lixeira. Essa ação não pode ser desfeita."],
    it: ["Verrà eliminato definitivamente {length} elemento selezionato dal cestino ({size}). Non è possibile annullare questa operazione.", "Verranno eliminati definitivamente {length} elementi selezionati dal cestino ({size}). Non è possibile annullare questa operazione."],
    ru: ["Из корзины будет безвозвратно удалён {length} выбранный элемент ({size}). Это действие нельзя отменить.", "Из корзины будут безвозвратно удалены {length} выбранных элемента ({size}). Это действие нельзя отменить.", "Из корзины будут безвозвратно удалены {length} выбранных элементов ({size}). Это действие нельзя отменить.", "Из корзины будет безвозвратно удалено {length} выбранного элемента ({size}). Это действие нельзя отменить."],
    ja: "ごみ箱で選択した {length} 件（{size}）を完全に削除します。この操作は取り消せません。",
    ko: "휴지통에서 선택한 항목 {length}개({size})를 영구적으로 삭제합니다. 이 작업은 취소할 수 없습니다.",
  }),
  [conversationCountSources.active]: countBy("length", {
    en: ["{length} active conversation", "{length} active conversations"], de: ["{length} aktive Unterhaltung", "{length} aktive Unterhaltungen"],
    fr: ["{length} conversation active", "{length} conversations actives"], es: ["{length} conversación activa", "{length} conversaciones activas"],
    "pt-BR": ["{length} conversa ativa", "{length} conversas ativas"], it: ["{length} conversazione attiva", "{length} conversazioni attive"],
    ru: ["{length} активный чат", "{length} активных чата", "{length} активных чатов", "{length} активного чата"],
    ja: "進行中の会話 {length} 件", ko: "활성 대화 {length}개",
  }),
  [conversationCountSources.archived]: countBy("length", {
    en: ["{length} archived conversation", "{length} archived conversations"], de: ["{length} archivierte Unterhaltung", "{length} archivierte Unterhaltungen"],
    fr: ["{length} conversation archivée", "{length} conversations archivées"], es: ["{length} conversación archivada", "{length} conversaciones archivadas"],
    "pt-BR": ["{length} conversa arquivada", "{length} conversas arquivadas"], it: ["{length} conversazione archiviata", "{length} conversazioni archiviate"],
    ru: ["{length} архивный чат", "{length} архивных чата", "{length} архивных чатов", "{length} архивного чата"],
    ja: "アーカイブ済みの会話 {length} 件", ko: "보관된 대화 {length}개",
  }),
  [conversationCountSources.group]: countBy("length", {
    en: ["{length} conversation", "{length} conversations"], de: ["{length} Unterhaltung", "{length} Unterhaltungen"],
    fr: ["{length} conversation", "{length} conversations"], es: ["{length} conversación", "{length} conversaciones"],
    "pt-BR": ["{length} conversa", "{length} conversas"], it: ["{length} conversazione", "{length} conversazioni"],
    ru: ["{length} чат", "{length} чата", "{length} чатов"], ja: "{length} 件の会話", ko: "대화 {length}개",
  }),
  "{message_count} 条消息": countBy("message_count", {
    en: ["{message_count} message", "{message_count} messages"], de: ["{message_count} Nachricht", "{message_count} Nachrichten"],
    fr: ["{message_count} message", "{message_count} messages"], es: ["{message_count} mensaje", "{message_count} mensajes"],
    "pt-BR": ["{message_count} mensagem", "{message_count} mensagens"], it: ["{message_count} messaggio", "{message_count} messaggi"],
    ru: ["{message_count} сообщение", "{message_count} сообщения", "{message_count} сообщений"],
  }),
  "{value1} 个管理中的游戏": countBy("value1", {
    en: ["{value1} managed game", "{value1} managed games"], de: ["{value1} verwaltetes Spiel", "{value1} verwaltete Spiele"],
    fr: ["{value1} jeu géré", "{value1} jeux gérés"], es: ["{value1} juego gestionado", "{value1} juegos gestionados"],
    "pt-BR": ["{value1} jogo gerenciado", "{value1} jogos gerenciados"], it: ["{value1} gioco gestito", "{value1} giochi gestiti"],
    ru: ["{value1} игра под управлением", "{value1} игры под управлением", "{value1} игр под управлением"],
  }),
  "{length} 款游戏": countBy("length", {
    en: ["{length} game", "{length} games"], de: ["{length} Spiel", "{length} Spiele"],
    fr: ["{length} jeu", "{length} jeux"], es: ["{length} juego", "{length} juegos"],
    "pt-BR": ["{length} jogo", "{length} jogos"], it: ["{length} gioco", "{length} giochi"],
    ru: ["{length} игра", "{length} игры", "{length} игр"],
  }),
  "已选 {length} 款游戏": countBy("length", {
    en: ["{length} game selected", "{length} games selected"], de: ["{length} Spiel ausgewählt", "{length} Spiele ausgewählt"],
    fr: ["{length} jeu sélectionné", "{length} jeux sélectionnés"], es: ["{length} juego seleccionado", "{length} juegos seleccionados"],
    "pt-BR": ["{length} jogo selecionado", "{length} jogos selecionados"], it: ["{length} gioco selezionato", "{length} giochi selezionati"],
    ru: ["Выбрана {length} игра", "Выбрано {length} игры", "Выбрано {length} игр"],
    ja: "ゲームを {length} 本選択済み", ko: "게임 {length}개 선택됨",
  }),
  "{platform} · {installedModCount} 个 Mod": countBy("installedModCount", {
    en: ["{platform} · {installedModCount} mod", "{platform} · {installedModCount} mods"], de: ["{platform} · {installedModCount} Mod", "{platform} · {installedModCount} Mods"],
    fr: ["{platform} · {installedModCount} mod", "{platform} · {installedModCount} mods"], es: ["{platform} · {installedModCount} mod", "{platform} · {installedModCount} mods"],
    "pt-BR": ["{platform} · {installedModCount} mod", "{platform} · {installedModCount} mods"], it: "{platform} · {installedModCount} mod",
    ru: ["{platform} · {installedModCount} мод", "{platform} · {installedModCount} мода", "{platform} · {installedModCount} модов"],
  }),
  "已安装 {value1} 个 Mod": countBy("value1", {
    en: ["{value1} mod installed", "{value1} mods installed"], de: ["{value1} Mod installiert", "{value1} Mods installiert"],
    fr: ["{value1} mod installé", "{value1} mods installés"], es: ["{value1} mod instalado", "{value1} mods instalados"],
    "pt-BR": ["{value1} mod instalado", "{value1} mods instalados"], it: ["{value1} mod installata", "{value1} mod installate"],
    ru: ["Установлен {value1} мод", "Установлено {value1} мода", "Установлено {value1} модов"],
  }),
  "{value1} {count} 个 Mod": countBy("count", {
    en: ["{value1}: {count} mod", "{value1}: {count} mods"], de: ["{value1}: {count} Mod", "{value1}: {count} Mods"],
    fr: ["{value1} : {count} mod", "{value1} : {count} mods"], es: ["{value1}: {count} mod", "{value1}: {count} mods"],
    "pt-BR": ["{value1}: {count} mod", "{value1}: {count} mods"], it: "{value1}: {count} mod",
    ru: ["{value1}: {count} мод", "{value1}: {count} мода", "{value1}: {count} модов"],
  }),
  "{value1} 个已安装插件": countBy("value1", {
    en: ["{value1} installed plugin", "{value1} installed plugins"], de: ["{value1} installiertes Plugin", "{value1} installierte Plugins"],
    fr: ["{value1} extension installée", "{value1} extensions installées"], es: ["{value1} complemento instalado", "{value1} complementos instalados"],
    "pt-BR": ["{value1} plugin instalado", "{value1} plugins instalados"], it: ["{value1} plugin installato", "{value1} plugin installati"],
    ru: ["{value1} установленный плагин", "{value1} установленных плагина", "{value1} установленных плагинов", "{value1} установленного плагина"],
  }),
  "草稿 {value1}": countBy("value1", {
    en: ["{value1} draft", "{value1} drafts"], de: ["{value1} Entwurf", "{value1} Entwürfe"],
    fr: ["{value1} brouillon", "{value1} brouillons"], es: ["{value1} borrador", "{value1} borradores"],
    "pt-BR": ["{value1} rascunho", "{value1} rascunhos"], it: ["{value1} bozza", "{value1} bozze"],
    ru: ["{value1} черновик", "{value1} черновика", "{value1} черновиков"], ja: "下書き {value1} 件", ko: "초안 {value1}개",
  }),
  "· {length} 个工具": countBy("length", {
    en: ["· {length} tool", "· {length} tools"], de: ["· {length} Werkzeug", "· {length} Werkzeuge"],
    fr: ["· {length} outil", "· {length} outils"], es: ["· {length} herramienta", "· {length} herramientas"],
    "pt-BR": ["· {length} ferramenta", "· {length} ferramentas"], it: ["· {length} strumento", "· {length} strumenti"],
    ru: ["· {length} инструмент", "· {length} инструмента", "· {length} инструментов"],
  }),
  "{length} 个文件": countBy("length", {
    en: ["{length} file", "{length} files"], de: ["{length} Datei", "{length} Dateien"],
    fr: ["{length} fichier", "{length} fichiers"], es: ["{length} archivo", "{length} archivos"],
    "pt-BR": ["{length} arquivo", "{length} arquivos"], it: "{length} file",
    ru: ["{length} файл", "{length} файла", "{length} файлов"],
  }),
  "{length} 项": items,
  "{length} 个": items,
  "已选 {length} 项": selectedItems,
  "已选择 {length} 项": selectedItems,
  "可多选 · 已选 {length} 项": countParts({
    en: "Multiple selections allowed · [length]", de: "Mehrfachauswahl möglich · [length]", fr: "Sélection multiple possible · [length]",
    es: "Selección múltiple disponible · [length]", "pt-BR": "Seleção múltipla disponível · [length]", it: "Selezione multipla disponibile · [length]",
    ru: "Можно выбрать несколько вариантов · [length]", ja: "複数選択可 · [length]", ko: "복수 선택 가능 · [length]",
  }, { length: {
    en: ["{length} option selected", "{length} options selected"], de: ["{length} Option ausgewählt", "{length} Optionen ausgewählt"],
    fr: ["{length} option sélectionnée", "{length} options sélectionnées"], es: ["{length} opción seleccionada", "{length} opciones seleccionadas"],
    "pt-BR": ["{length} opção selecionada", "{length} opções selecionadas"], it: ["{length} opzione selezionata", "{length} opzioni selezionate"],
    ru: ["Выбран {length} вариант", "Выбрано {length} варианта", "Выбрано {length} вариантов"], ja: "{length} 件選択済み", ko: "{length}개 선택됨",
  } }),
  "{value1}：正在提交 {length} 项": countBy("length", {
    en: ["{value1}: submitting {length} item…", "{value1}: submitting {length} items…"], de: ["{value1}: {length} Eintrag wird übermittelt…", "{value1}: {length} Einträge werden übermittelt…"],
    fr: ["{value1} : envoi de {length} élément…", "{value1} : envoi de {length} éléments…"], es: ["{value1}: enviando {length} elemento…", "{value1}: enviando {length} elementos…"],
    "pt-BR": ["{value1}: enviando {length} item…", "{value1}: enviando {length} itens…"], it: ["{value1}: invio di {length} elemento…", "{value1}: invio di {length} elementi…"],
    ru: ["{value1}: отправляется {length} элемент…", "{value1}: отправляются {length} элемента…", "{value1}: отправляется {length} элементов…", "{value1}: отправляется {length} элемента…"],
  }),
  "已提交 1 个 Agent 任务（{length} 项）": countBy("length", {
    en: ["Submitted one agent task ({length} item)", "Submitted one agent task ({length} items)"], de: ["Ein Agent-Auftrag übermittelt ({length} Eintrag)", "Ein Agent-Auftrag übermittelt ({length} Einträge)"],
    fr: ["Une tâche agent envoyée ({length} élément)", "Une tâche agent envoyée ({length} éléments)"], es: ["Una tarea del agente enviada ({length} elemento)", "Una tarea del agente enviada ({length} elementos)"],
    "pt-BR": ["Uma tarefa do agente enviada ({length} item)", "Uma tarefa do agente enviada ({length} itens)"], it: ["Un'attività dell'agente inviata ({length} elemento)", "Un'attività dell'agente inviata ({length} elementi)"],
    ru: ["Отправлена одна задача агента ({length} элемент)", "Отправлена одна задача агента ({length} элемента)", "Отправлена одна задача агента ({length} элементов)"],
  }),
  "已删除 {length} 项": countBy("length", {
    en: ["Deleted {length} item", "Deleted {length} items"], de: ["{length} Eintrag gelöscht", "{length} Einträge gelöscht"],
    fr: ["{length} élément supprimé", "{length} éléments supprimés"], es: ["{length} elemento eliminado", "{length} elementos eliminados"],
    "pt-BR": ["{length} item excluído", "{length} itens excluídos"], it: ["{length} elemento eliminato", "{length} elementi eliminati"],
    ru: ["Удалён {length} элемент", "Удалено {length} элемента", "Удалено {length} элементов"],
  }),
  "跳过 {skipped} 项": countBy("skipped", {
    en: ["Skipped {skipped} item", "Skipped {skipped} items"], de: ["{skipped} Eintrag übersprungen", "{skipped} Einträge übersprungen"],
    fr: ["{skipped} élément ignoré", "{skipped} éléments ignorés"], es: ["{skipped} elemento omitido", "{skipped} elementos omitidos"],
    "pt-BR": ["{skipped} item ignorado", "{skipped} itens ignorados"], it: ["{skipped} elemento ignorato", "{skipped} elementi ignorati"],
    ru: ["Пропущен {skipped} элемент", "Пропущено {skipped} элемента", "Пропущено {skipped} элементов"],
  }),
  "{length} 项失败": countBy("length", {
    en: ["{length} item failed", "{length} items failed"], de: ["{length} Eintrag fehlgeschlagen", "{length} Einträge fehlgeschlagen"],
    fr: ["{length} élément en échec", "{length} éléments en échec"], es: ["{length} elemento con error", "{length} elementos con errores"],
    "pt-BR": ["{length} item com falha", "{length} itens com falha"], it: ["{length} elemento non riuscito", "{length} elementi non riusciti"],
    ru: ["Ошибка: {length} элемент", "Ошибка: {length} элемента", "Ошибка: {length} элементов"],
  }),
  "{value1}：{length} 项可执行{value3}": countBy("length", {
    en: ["{value1}: {length} eligible item{value3}", "{value1}: {length} eligible items{value3}"], de: ["{value1}: {length} geeigneter Eintrag{value3}", "{value1}: {length} geeignete Einträge{value3}"],
    fr: ["{value1} : {length} élément admissible{value3}", "{value1} : {length} éléments admissibles{value3}"], es: ["{value1}: {length} elemento apto{value3}", "{value1}: {length} elementos aptos{value3}"],
    "pt-BR": ["{value1}: {length} item elegível{value3}", "{value1}: {length} itens elegíveis{value3}"], it: ["{value1}: {length} elemento idoneo{value3}", "{value1}: {length} elementi idonei{value3}"],
    ru: ["{value1}: {length} подходящий элемент{value3}", "{value1}: {length} подходящих элемента{value3}", "{value1}: {length} подходящих элементов{value3}", "{value1}: {length} подходящего элемента{value3}"],
    ja: "{value1}：対象 {length} 件{value3}", ko: "{value1}: 적용 가능 {length}개{value3}",
  }),
  "，{skipped} 项不适用": countBy("skipped", {
    en: ", {skipped} not applicable", de: ", {skipped} nicht zutreffend",
    fr: [", {skipped} non admissible", ", {skipped} non admissibles"], es: [", {skipped} no apto", ", {skipped} no aptos"],
    "pt-BR": [", {skipped} não elegível", ", {skipped} não elegíveis"], it: [", {skipped} non idoneo", ", {skipped} non idonei"],
    ru: [", не подходит: {skipped}", ", не подходят: {skipped}", ", не подходят: {skipped}"],
  }),
  "将从此游戏的管理列表删除 {length} 项。": countBy("length", {
    en: ["This will remove {length} item from this game's managed list.", "This will remove {length} items from this game's managed list."], de: ["{length} Eintrag wird aus der Verwaltungsliste dieses Spiels entfernt.", "{length} Einträge werden aus der Verwaltungsliste dieses Spiels entfernt."],
    fr: ["{length} élément sera retiré de la liste de gestion de ce jeu.", "{length} éléments seront retirés de la liste de gestion de ce jeu."], es: ["Se eliminará {length} elemento de la lista de gestión de este juego.", "Se eliminarán {length} elementos de la lista de gestión de este juego."],
    "pt-BR": ["{length} item será removido da lista de gerenciamento deste jogo.", "{length} itens serão removidos da lista de gerenciamento deste jogo."], it: ["Verrà rimosso {length} elemento dall'elenco di gestione di questo gioco.", "Verranno rimossi {length} elementi dall'elenco di gestione di questo gioco."],
    ru: ["Из списка управления игрой будет удалён {length} элемент.", "Из списка управления игрой будут удалены {length} элемента.", "Из списка управления игрой будет удалено {length} элементов.", "Из списка управления игрой будет удалено {length} элемента."],
  }),
  "另有 {skipped} 项不适用，将跳过。": countBy("skipped", {
    en: ["{skipped} other item is not applicable and will be skipped.", "{skipped} other items are not applicable and will be skipped."], de: ["{skipped} weiterer Eintrag ist nicht betroffen und wird übersprungen.", "{skipped} weitere Einträge sind nicht betroffen und werden übersprungen."],
    fr: ["{skipped} autre élément non admissible sera ignoré.", "{skipped} autres éléments non admissibles seront ignorés."], es: ["Se omitirá {skipped} elemento adicional no apto.", "Se omitirán {skipped} elementos adicionales no aptos."],
    "pt-BR": ["{skipped} outro item não se aplica e será ignorado.", "{skipped} outros itens não se aplicam e serão ignorados."], it: ["{skipped} altro elemento non idoneo verrà ignorato.", "{skipped} altri elementi non idonei verranno ignorati."],
    ru: ["Ещё {skipped} неподходящий элемент будет пропущен.", "Ещё {skipped} неподходящих элемента будут пропущены.", "Ещё {skipped} неподходящих элементов будут пропущены.", "Ещё {skipped} неподходящего элемента будет пропущено."],
  }),
  "已检查 {directoriesChecked} 个目录、{entriesChecked} 个条目": countParts({
    en: "Checked: [directoriesChecked], [entriesChecked]", de: "Geprüft: [directoriesChecked], [entriesChecked]",
    fr: "Vérification : [directoriesChecked], [entriesChecked]", es: "Revisados: [directoriesChecked], [entriesChecked]",
    "pt-BR": "Verificação: [directoriesChecked], [entriesChecked]", it: "Verifica: [directoriesChecked], [entriesChecked]",
    ru: "Проверено: [directoriesChecked], [entriesChecked]", ja: "確認済み：[directoriesChecked]、[entriesChecked]",
    ko: "확인 완료: [directoriesChecked], [entriesChecked]",
  }, scanCounts),
  "扫描警告（{length}）": countBy("length", {
    en: ["{length} scan warning", "{length} scan warnings"], de: ["{length} Scanwarnung", "{length} Scanwarnungen"],
    fr: ["{length} avertissement d’analyse", "{length} avertissements d’analyse"], es: ["{length} advertencia de escaneo", "{length} advertencias de escaneo"],
    "pt-BR": ["{length} aviso de verificação", "{length} avisos de verificação"], it: ["{length} avviso di scansione", "{length} avvisi di scansione"],
    ru: ["{length} предупреждение при сканировании", "{length} предупреждения при сканировании", "{length} предупреждений при сканировании"],
  }),
  "已检查 {directoriesChecked} 个目录、{entriesChecked} 个条目，本批 {length} 个候选目录{value4}": countParts({
    en: "Checked: [directoriesChecked], [entriesChecked] · This batch: [length]{value4}",
    de: "Geprüft: [directoriesChecked], [entriesChecked] · Dieser Durchlauf: [length]{value4}",
    fr: "Vérification : [directoriesChecked], [entriesChecked] · Dans ce lot : [length]{value4}",
    es: "Revisados: [directoriesChecked], [entriesChecked] · En este lote: [length]{value4}",
    "pt-BR": "Verificação: [directoriesChecked], [entriesChecked] · Neste lote: [length]{value4}",
    it: "Verifica: [directoriesChecked], [entriesChecked] · In questo lotto: [length]{value4}",
    ru: "Проверено: [directoriesChecked], [entriesChecked] · В этой партии: [length]{value4}",
    ja: "確認済み：[directoriesChecked]、[entriesChecked] · 今回：[length]{value4}",
    ko: "확인 완료: [directoriesChecked], [entriesChecked] · 이번 검색: [length]{value4}",
  }, {
    ...scanCounts,
    length: {
      en: ["{length} candidate directory", "{length} candidate directories"], de: ["{length} möglicher Spieleordner", "{length} mögliche Spieleordner"],
      fr: ["{length} dossier candidat", "{length} dossiers candidats"], es: ["{length} directorio candidato", "{length} directorios candidatos"],
      "pt-BR": ["{length} diretório candidato", "{length} diretórios candidatos"], it: ["{length} directory candidata", "{length} directory candidate"],
      ru: ["{length} подходящий каталог", "{length} подходящих каталога", "{length} подходящих каталогов", "{length} подходящего каталога"], ja: "候補ディレクトリ {length} 件", ko: "후보 디렉터리 {length}개",
    },
  }),
  "{length} 项 · {value2}": countBy("length", {
    en: ["{length} item · {value2}", "{length} items · {value2}"], de: ["{length} Eintrag · {value2}", "{length} Einträge · {value2}"],
    fr: ["{length} élément · {value2}", "{length} éléments · {value2}"], es: ["{length} elemento · {value2}", "{length} elementos · {value2}"],
    "pt-BR": ["{length} item · {value2}", "{length} itens · {value2}"], it: ["{length} elemento · {value2}", "{length} elementi · {value2}"],
    ru: ["{length} элемент · {value2}", "{length} элемента · {value2}", "{length} элементов · {value2}"],
  }),
  "已永久删除 {length} 项，释放 {value2}": countBy("length", {
    en: ["Permanently deleted {length} item, freeing {value2}", "Permanently deleted {length} items, freeing {value2}"],
    de: ["{length} Eintrag endgültig gelöscht · {value2} freigegeben", "{length} Einträge endgültig gelöscht · {value2} freigegeben"],
    fr: ["{length} élément définitivement supprimé · espace libéré : {value2}", "{length} éléments définitivement supprimés · espace libéré : {value2}"],
    es: ["{length} elemento eliminado definitivamente · espacio liberado: {value2}", "{length} elementos eliminados definitivamente · espacio liberado: {value2}"],
    "pt-BR": ["{length} item excluído permanentemente · espaço liberado: {value2}", "{length} itens excluídos permanentemente · espaço liberado: {value2}"],
    it: ["{length} elemento eliminato definitivamente · spazio liberato: {value2}", "{length} elementi eliminati definitivamente · spazio liberato: {value2}"],
    ru: ["Безвозвратно удалён {length} элемент · освобождено {value2}", "Безвозвратно удалено {length} элемента · освобождено {value2}", "Безвозвратно удалено {length} элементов · освобождено {value2}"],
  }),
  "显示 {value1}–{value2} 项，共 {total} 项": countBy("total", {
    en: "Showing {value1}–{value2} of {total}", de: "{value1}–{value2} von {total}", fr: "Affichage de {value1} à {value2} sur {total}",
    es: "Mostrando del {value1} al {value2} de {total}", "pt-BR": "Exibindo de {value1} a {value2} de {total}", it: "Da {value1} a {value2} di {total}",
    ru: "Показано: {value1}–{value2} из {total}",
  }),
  "第 {page} 页，共 {pages} 页": countBy("pages", {
    en: "Page {page} of {pages}", de: "Seite {page} von {pages}", fr: "Page {page} sur {pages}", es: "Página {page} de {pages}",
    "pt-BR": "Página {page} de {pages}", it: "Pagina {page} di {pages}", ru: "Страница {page} из {pages}",
  }),
  "{size} 项 / 页": countBy("size", {
    en: ["{size} item per page", "{size} items per page"], de: ["{size} Eintrag pro Seite", "{size} Einträge pro Seite"],
    fr: ["{size} élément par page", "{size} éléments par page"], es: ["{size} elemento por página", "{size} elementos por página"],
    "pt-BR": ["{size} item por página", "{size} itens por página"], it: ["{size} elemento per pagina", "{size} elementi per pagina"],
    ru: ["{size} элемент на странице", "{size} элемента на странице", "{size} элементов на странице"],
  }),
  "第 {value1} / {candidatePages} 页，共 {length} 个模型": countBy("length", {
    en: ["Page {value1} of {candidatePages} · {length} model", "Page {value1} of {candidatePages} · {length} models"],
    de: ["Seite {value1} von {candidatePages} · {length} Modell", "Seite {value1} von {candidatePages} · {length} Modelle"],
    fr: ["Page {value1} sur {candidatePages} · {length} modèle", "Page {value1} sur {candidatePages} · {length} modèles"],
    es: ["Página {value1} de {candidatePages} · {length} modelo", "Página {value1} de {candidatePages} · {length} modelos"],
    "pt-BR": ["Página {value1} de {candidatePages} · {length} modelo", "Página {value1} de {candidatePages} · {length} modelos"],
    it: ["Pagina {value1} di {candidatePages} · {length} modello", "Pagina {value1} di {candidatePages} · {length} modelli"],
    ru: ["Страница {value1} из {candidatePages} · {length} модель", "Страница {value1} из {candidatePages} · {length} модели", "Страница {value1} из {candidatePages} · {length} моделей"],
  }),
  "第 {value1} / {optionPages} 页，共 {length} 个选项": countBy("length", {
    en: ["Page {value1} of {optionPages} · {length} option", "Page {value1} of {optionPages} · {length} options"],
    de: ["Seite {value1} von {optionPages} · {length} Option", "Seite {value1} von {optionPages} · {length} Optionen"],
    fr: ["Page {value1} sur {optionPages} · {length} option", "Page {value1} sur {optionPages} · {length} options"],
    es: ["Página {value1} de {optionPages} · {length} opción", "Página {value1} de {optionPages} · {length} opciones"],
    "pt-BR": ["Página {value1} de {optionPages} · {length} opção", "Página {value1} de {optionPages} · {length} opções"],
    it: ["Pagina {value1} di {optionPages} · {length} opzione", "Pagina {value1} di {optionPages} · {length} opzioni"],
    ru: ["Страница {value1} из {optionPages} · {length} вариант", "Страница {value1} из {optionPages} · {length} варианта", "Страница {value1} из {optionPages} · {length} вариантов"],
  }),
  "第 {value1} 题，共 {length} 题": countBy("length", {
    en: "Question {value1} of {length}", de: "Frage {value1} von {length}", fr: "Question {value1} sur {length}", es: "Pregunta {value1} de {length}",
    "pt-BR": "Pergunta {value1} de {length}", it: "Domanda {value1} di {length}", ru: "Вопрос {value1} из {length}",
  }),
  "已检查 {length} 个启用插件": countBy("length", {
    en: ["Checked {length} enabled plugin", "Checked {length} enabled plugins"], de: ["{length} aktiviertes Plugin geprüft", "{length} aktivierte Plugins geprüft"],
    fr: ["{length} extension active vérifiée", "{length} extensions actives vérifiées"], es: ["{length} complemento activo verificado", "{length} complementos activos verificados"],
    "pt-BR": ["{length} plugin ativo verificado", "{length} plugins ativos verificados"], it: ["{length} plugin attivo verificato", "{length} plugin attivi verificati"],
    ru: ["Проверен {length} активный плагин", "Проверено {length} активных плагина", "Проверено {length} активных плагинов", "Проверено {length} активного плагина"],
  }),
  "添加所选（{size}）": countBy("size", {
    en: "Add selected ({size})", de: "Auswahl hinzufügen ({size})", fr: "Ajouter la sélection ({size})", es: "Añadir selección ({size})",
    "pt-BR": "Adicionar seleção ({size})", it: "Aggiungi selezione ({size})", ru: "Добавить выбранное ({size})",
  }),
  "已询问 {count} 个问题": countBy("count", {
    en: ["Asked {count} question", "Asked {count} questions"], de: ["{count} Frage gestellt", "{count} Fragen gestellt"],
    fr: ["{count} question posée", "{count} questions posées"], es: ["{count} pregunta formulada", "{count} preguntas formuladas"],
    "pt-BR": ["{count} pergunta feita", "{count} perguntas feitas"], it: ["{count} domanda posta", "{count} domande poste"],
    ru: ["Задан {count} вопрос", "Задано {count} вопроса", "Задано {count} вопросов"],
  }),
  "目录中有 {skippedEntries} 个无效条目，已跳过。": countBy("skippedEntries", {
    en: ["Skipped {skippedEntries} invalid catalog entry.", "Skipped {skippedEntries} invalid catalog entries."],
    de: ["{skippedEntries} ungültiger Katalogeintrag wurde übersprungen.", "{skippedEntries} ungültige Katalogeinträge wurden übersprungen."],
    fr: ["{skippedEntries} entrée de catalogue non valide a été ignorée.", "{skippedEntries} entrées de catalogue non valides ont été ignorées."],
    es: ["Se omitió {skippedEntries} entrada no válida del catálogo.", "Se omitieron {skippedEntries} entradas no válidas del catálogo."],
    "pt-BR": ["{skippedEntries} entrada inválida do catálogo foi ignorada.", "{skippedEntries} entradas inválidas do catálogo foram ignoradas."],
    it: ["È stata ignorata {skippedEntries} voce non valida del catalogo.", "Sono state ignorate {skippedEntries} voci non valide del catalogo."],
    ru: ["Пропущена {skippedEntries} недопустимая запись в каталоге.", "Пропущено {skippedEntries} недопустимые записи в каталоге.", "Пропущено {skippedEntries} недопустимых записей в каталоге.", "Пропущено {skippedEntries} недопустимой записи в каталоге."],
  }),
  "已{value1} {succeeded} 个连接{value3}": countBy("succeeded", {
    en: ["{value1}: {succeeded} connection updated{value3}", "{value1}: {succeeded} connections updated{value3}"],
    de: ["{value1}: {succeeded} Verbindung aktualisiert{value3}", "{value1}: {succeeded} Verbindungen aktualisiert{value3}"],
    fr: ["{value1} : {succeeded} connexion mise à jour{value3}", "{value1} : {succeeded} connexions mises à jour{value3}"],
    es: ["{value1}: {succeeded} conexión actualizada{value3}", "{value1}: {succeeded} conexiones actualizadas{value3}"],
    "pt-BR": ["{value1}: {succeeded} conexão atualizada{value3}", "{value1}: {succeeded} conexões atualizadas{value3}"],
    it: ["{value1}: {succeeded} connessione aggiornata{value3}", "{value1}: {succeeded} connessioni aggiornate{value3}"],
    ru: ["{value1}: обновлено {succeeded} подключение{value3}", "{value1}: обновлено {succeeded} подключения{value3}", "{value1}: обновлено {succeeded} подключений{value3}"],
    ja: "{value1}：接続 {succeeded} 件を更新{value3}", ko: "{value1}: 연결 {succeeded}개 업데이트{value3}",
  }),
  "，{failed} 个失败": countBy("failed", {
    en: ", {failed} failed", de: ", fehlgeschlagen: {failed}", fr: ", échecs : {failed}", es: ", errores: {failed}",
    "pt-BR": ", falhas: {failed}", it: ", errori: {failed}", ru: ", ошибок: {failed}",
  }),
};

export function pluralTemplate(locale: Locale, source: string, values: Values) {
  if (locale === "zh-CN" || !Object.hasOwn(templates, source)) return;
  return templates[source](locale, values);
}
