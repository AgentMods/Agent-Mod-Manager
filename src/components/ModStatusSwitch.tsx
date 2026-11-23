import { useId } from "react";
import type { PendingModSwitch } from "@/lib/mod-switch-store";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";

export function ModStatusSwitch({ checked, action, status, pending, disabled, hint, onToggle }: {
  checked: boolean;
  action: string;
  status: string;
  pending?: PendingModSwitch;
  disabled: boolean;
  hint?: string;
  onToggle: () => void;
}) {
  const description = useId();
  const loading = !!pending && pending.status !== "readback_failed";
  const label = pending?.status === "readback_failed" ? "状态待核实"
    : loading ? (pending.kind === "install" ? "安装中…" : pending.kind === "remove" ? "删除中…" : "卸载中…") : status;
  return <div className="inline-flex min-h-8 items-center gap-2 whitespace-nowrap" title={hint ?? action}>
    <span className="relative inline-flex items-center justify-center">
      <Switch checked={checked} disabled={disabled || !!pending} onCheckedChange={onToggle}
        aria-label={action} aria-busy={loading} aria-describedby={description}
        className={loading ? "data-checked:bg-muted data-unchecked:bg-muted [&_[data-slot=switch-thumb]]:invisible" : undefined} />
      {loading && <Spinner className="pointer-events-none absolute size-3 text-foreground motion-reduce:animate-none" />}
    </span>
    <span id={description} className="text-sm">{label}</span>
  </div>;
}
