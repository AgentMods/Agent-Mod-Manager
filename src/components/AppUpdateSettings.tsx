import { tr } from "@/lib/i18n";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { RefreshCwIcon } from "lucide-react";
import { api, backendAvailable, useBackendQuery } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card, CardAction, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import { FieldError } from "@/components/ui/field";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Spinner } from "@/components/ui/spinner";

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

  return <Card>
    <CardHeader>
      <CardTitle>{tr("应用更新")}</CardTitle>
      <CardDescription role="status">{message}</CardDescription>
      <CardAction>
        <Button variant="outline" disabled={!backendAvailable || saveRoute.isPending || busy || state === "ready" || state === "unsupported"} onClick={() => check.mutate()}>
          {busy ? <Spinner data-icon="inline-start" /> : <RefreshCwIcon data-icon="inline-start" />}{tr("检查更新")}
        </Button>
      </CardAction>
    </CardHeader>
    <CardContent className="grid gap-2">
      <div className="flex flex-wrap items-center gap-3">
        <Label htmlFor="app-update-route">{tr("下载线路")}</Label>
        <NativeSelect id="app-update-route" value={(saveRoute.isPending ? saveRoute.variables : route.data) ?? ""}
          disabled={!backendAvailable || !route.data || saveRoute.isPending}
          aria-describedby="app-update-route-help" onChange={(event) => saveRoute.mutate(event.target.value)}>
          {!route.data && <NativeSelectOption value="" disabled>—</NativeSelectOption>}
          {proxyHosts.map((host) => <NativeSelectOption key={host} value={host}>{host}</NativeSelectOption>)}
          <NativeSelectOption value="direct">{tr("GitHub 直连")}</NativeSelectOption>
        </NativeSelect>
      </div>
      {routeError && <FieldError>{routeError.message}</FieldError>}
    </CardContent>
  </Card>;
}
