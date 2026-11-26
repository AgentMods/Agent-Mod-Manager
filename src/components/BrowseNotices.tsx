import { useState } from "react";
import { AlertCircleIcon, ChevronDownIcon } from "lucide-react";
import { tr } from "@/lib/i18n";
import { issueText, type BrowseChoice, type BrowseIssue } from "@/lib/browse";
import { Alert } from "./ui/alert";
import { NativeSelect } from "./ui/native-select";

export function BrowseNotices({
  issues,
  choices,
  gameName,
  sourceName,
  select,
}: {
  issues: BrowseIssue[];
  choices: BrowseChoice[];
  gameName: (id: string) => string;
  sourceName: (id: string) => string;
  select: (choice: BrowseChoice, id: string) => Promise<void>;
}) {
  const [saving, setSaving] = useState(false);
  if (!issues.length && !choices.length) return null;
  return (
    <Alert variant="warning" className="block p-0">
      <details className="group/notices">
        <summary className="flex cursor-pointer list-none items-center gap-2 rounded-lg px-4 py-2.5 font-medium focus-visible:outline-2 focus-visible:outline-ring [&::-webkit-details-marker]:hidden">
          <AlertCircleIcon aria-hidden="true" className="size-4 shrink-0" />
          {tr("来源匹配提示（{count}）", { count: issues.length + choices.length })}
          <ChevronDownIcon aria-hidden="true" className="ml-auto size-4 shrink-0 group-open/notices:rotate-180" />
        </summary>
        <div className="flex min-w-0 flex-col items-start gap-2 border-t border-warning/20 px-4 py-3 text-foreground/80">
          {issues.map((i) => (
            <span className="break-words" key={`${i.gameId}/${i.sourceId}/${i.code}`}>
              {gameName(i.gameId)} · {sourceName(i.sourceId)} — {issueText(i.code)}
            </span>
          ))}
          {choices.map((c) => (
            <label className="flex flex-wrap items-center gap-2" key={`${c.gameId}/${c.sourceId}`}>
              <span>
                {gameName(c.gameId)} · {sourceName(c.sourceId)}
              </span>
              <NativeSelect
                disabled={saving}
                aria-label={tr("匹配来源游戏")}
                value=""
                onChange={async (e) => {
                  const id = e.target.value;
                  if (!id || saving) return;
                  setSaving(true);
                  try {
                    await select(c, id);
                  } finally {
                    setSaving(false);
                  }
                }}
              >
                <option value="" disabled>
                  {tr("匹配来源游戏")}
                </option>
                {c.options.map((o) => (
                  <option key={o.id} value={o.id}>
                    {o.name} ({o.id})
                  </option>
                ))}
              </NativeSelect>
            </label>
          ))}
        </div>
      </details>
    </Alert>
  );
}
