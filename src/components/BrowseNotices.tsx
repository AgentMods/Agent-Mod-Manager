import { useState } from "react";
import { AlertCircleIcon, CompassIcon, ShieldCheckIcon } from "lucide-react";
import { tr } from "@/lib/i18n";
import { issueText, sourceStatus, type BrowseChoice, type BrowseIssue, type BrowseSource } from "@/lib/browse";
import { Alert, AlertDescription } from "./ui/alert";
import { Button } from "./ui/button";
import { NativeSelect } from "./ui/native-select";
import { Badge } from "./ui/badge";
import { EmptyState } from "./ui";

export function BrowseSourceGate({
  sources,
  allow,
  busy,
  settings,
  refresh,
}: {
  sources: BrowseSource[];
  allow: (ids: string[]) => void;
  busy: boolean;
  settings: () => void;
  refresh: () => void;
}) {
  const available = sources.filter((s) => s.status === "available");
  const [omitted, setOmitted] = useState<string[]>([]);
  const included = available.filter((s) => !omitted.includes(s.id)).map((s) => s.id);
  return (
    <EmptyState
      icon={available.length ? <ShieldCheckIcon /> : <CompassIcon />}
      title={available.length ? tr("允许查询这些 Mod 来源？") : tr("没有可用的 Mod 来源")}
      action={
        <div className="flex flex-col items-center gap-3">
          <div className="flex flex-wrap justify-center gap-2">
            {sources.map((s) =>
              s.status === "available" ? (
                <label key={s.id} className="flex items-center gap-2 rounded-md border px-2 py-1 text-xs">
                  <input
                    type="checkbox"
                    checked={!omitted.includes(s.id)}
                    disabled={busy}
                    onChange={(e) => {
                      const checked = e.currentTarget.checked;
                      setOmitted((v) => (checked ? v.filter((x) => x !== s.id) : [...v, s.id]));
                    }}
                  />
                  {s.name}
                </label>
              ) : (
                <Badge variant="outline" key={s.id}>
                  {s.name} · {sourceStatus(s.status)}
                </Badge>
              ),
            )}
          </div>
          <div className="flex flex-wrap justify-center gap-2">
            {available.length > 0 && (
              <Button disabled={!included.length || busy} onClick={() => allow(included)}>
                {tr("允许本次游览")}
              </Button>
            )}
            <Button variant="outline" onClick={settings}>
              {tr("设置")}
            </Button>
            <Button variant="ghost" disabled={busy} onClick={refresh}>
              {tr("刷新")}
            </Button>
          </div>
        </div>
      }
    >
      {available.length
        ? tr("只查询所选来源的公开目录，不会授权下载或安装。Agent 会用配置的模型整理公开资料和搜索词。")
        : tr("请在「设置 → 工具」连接 Mod 来源，然后刷新。")}
    </EmptyState>
  );
}

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
    <Alert variant="warning">
      <AlertCircleIcon />
      <AlertDescription className="flex min-w-0 flex-col items-start gap-2">
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
      </AlertDescription>
    </Alert>
  );
}
