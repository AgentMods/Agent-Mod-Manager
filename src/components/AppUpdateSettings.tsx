import { tr } from "@/lib/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowDownToLineIcon, GlobeIcon, RefreshCwIcon } from "lucide-react";
import { api, backendAvailable, useBackendQuery } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { FieldError } from "@/components/ui/field";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Spinner } from "@/components/ui/spinner";
import { cn } from "@/lib/utils";

type UpdateStatus = {
  status: "idle" | "checking" | "downloading" | "ready" | "upToDate" | "error" | "unsupported";
  version?: string;
  message?: string;
};

const queryKey = ["app-update"];
const routeKey = ["app-update-route"];
const proxyHosts = ["cdn.gh-proxy.org", "gh-proxy.org", "ghfast.top", "gh-proxy.com", "ghproxy.net"];

function describeUpdate(update?: UpdateStatus) {
  const version = update?.version ? ` ${update.version}` : "";
  switch (update?.status) {
    case "checking": return tr("正在检查更新…");
    case "downloading": return tr("正在后台下载新版本{version}…", { version });
    case "ready": return tr("新版本{version} 已下载，关闭程序后将自动安装。", { version });
    case "upToDate": return tr("当前已是最新版本。");
    case "error": return update.message || tr("检查更新失败，请稍后重试。");
    case "unsupported": return update.message || tr("当前安装方式不支持自动更新。");
    default: return tr("启动时自动检查并下载新版本，关闭程序后安装。");
  }
}

export function AppUpdateSettings() {
  const client = useQueryClient();
  const status = useQuery({
    queryKey,
    queryFn: () => api<UpdateStatus>("get_app_update_status"),
    enabled: backendAvailable,
    retry: false,
    refetchInterval: ({ state }) => ["checking", "downloading"].includes(state.data?.status ?? "") ? 1000 : false,
  });
  const route = useBackendQuery<string>(routeKey, "get_app_update_route");
  const saveRoute = useMutation({
    mutationFn: (value: string) => api<string>("set_app_update_route", { route: value }),
    onMutate: () => client.cancelQueries({ queryKey: routeKey }),
    onSuccess: (value) => client.setQueryData(routeKey, value),
  });
  const routeError = saveRoute.error ?? route.error;
  const check = useMutation({
    mutationFn: () => api<UpdateStatus>("check_app_update"),
    onMutate: () => client.cancelQueries({ queryKey }),
    onSuccess: (result) => client.setQueryData(queryKey, result),
  });
  const state = status.data?.status;
  const busy = check.isPending || state === "checking" || state === "downloading";
  const message = !backendAvailable ? tr("请在桌面应用中检查更新。")
    : check.isPending ? tr("正在检查更新…")
      : check.error?.message || status.error?.message || describeUpdate(status.data);
  const healthy = state === "upToDate" || state === "ready";
  const failed = Boolean(check.error || status.error || state === "error");

  return <Card className="surface-panel-card group/card gap-3 rounded-2xl py-3.5 shadow-xs">
    <CardHeader className="gap-1 px-4 sm:px-5">
      <CardTitle className="flex items-center gap-2 text-sm font-semibold sm:text-base">
        <span aria-hidden="true" className={cn(
          "flex size-7 shrink-0 items-center justify-center rounded-lg border motion-safe:transition-transform motion-safe:duration-300 group-hover/card:scale-110",
          failed ? "bg-warning/15 text-warning" : healthy ? "bg-success/15 text-success" : "bg-muted/60 text-foreground/85",
        )}>
          <ArrowDownToLineIcon className="size-3.5" />
        </span>
        {tr("应用更新")}
      </CardTitle>
      <CardDescription role="status" className="pl-9 text-xs">{message}</CardDescription>
      <CardAction>
        <Button
          variant="outline"
          size="sm"
          className="group/check bg-background/65 hover:bg-muted/60"
          disabled={!backendAvailable || saveRoute.isPending || busy || state === "ready" || state === "unsupported"}
          onClick={() => check.mutate()}
        >
          {busy
            ? <Spinner data-icon="inline-start" />
            : <RefreshCwIcon data-icon="inline-start" className="motion-safe:transition-transform motion-safe:duration-300 group-hover/check:rotate-90" />}
          {tr("检查更新")}
        </Button>
      </CardAction>
    </CardHeader>
    <CardContent className="grid gap-2 px-4 sm:px-5">
      <div className="flex flex-wrap items-center justify-between gap-2.5 rounded-xl border bg-muted/25 px-3.5 py-2.5 transition-colors duration-200 hover:border-foreground/20 [&_[data-slot=native-select-wrapper]]:w-full sm:[&_[data-slot=native-select-wrapper]]:w-fit sm:[&_[data-slot=native-select-wrapper]]:max-w-full">
        <div className="flex items-center gap-2">
          <GlobeIcon aria-hidden="true" className="size-4 text-muted-foreground" />
          <Label htmlFor="app-update-route">{tr("下载线路")}</Label>
        </div>
        <NativeSelect id="app-update-route" size="sm" value={(saveRoute.isPending ? saveRoute.variables : route.data) ?? ""}
          disabled={!backendAvailable || !route.data || saveRoute.isPending}
          aria-describedby="app-update-route-help" onChange={(event) => saveRoute.mutate(event.target.value)}>
          {!route.data && <NativeSelectOption value="" disabled>—</NativeSelectOption>}
          {proxyHosts.map((host) => <NativeSelectOption key={host} value={host}>{host}</NativeSelectOption>)}
          <NativeSelectOption value="direct">{tr("GitHub 直连")}</NativeSelectOption>
        </NativeSelect>
      </div>
      {routeError && <FieldError className="text-xs">{routeError.message}</FieldError>}
    </CardContent>
  </Card>;
}
