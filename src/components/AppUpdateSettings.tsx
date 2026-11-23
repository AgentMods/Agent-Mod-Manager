import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { RefreshCwIcon } from "lucide-react";
import { api, backendAvailable } from "@/lib/api";
import { Button } from "@/components/ui/button";
import { Card, CardAction, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { Spinner } from "@/components/ui/spinner";

type UpdateStatus = {
  status: "idle" | "checking" | "downloading" | "ready" | "upToDate" | "error" | "unsupported";
  version?: string;
  message?: string;
};

const queryKey = ["app-update"];

function describeUpdate(update?: UpdateStatus) {
  const version = update?.version ? ` ${update.version}` : "";
  switch (update?.status) {
    case "checking": return "正在检查更新…";
    case "downloading": return `正在后台下载新版本${version}…`;
    case "ready": return `新版本${version} 已下载，关闭程序后将自动安装。`;
    case "upToDate": return "当前已是最新版本。";
    case "error": return update.message || "检查更新失败，请稍后重试。";
    case "unsupported": return update.message || "当前安装方式不支持自动更新。";
    default: return "启动时自动检查并下载新版本，关闭程序后安装。";
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
  const check = useMutation({
    mutationFn: () => api<UpdateStatus>("check_app_update"),
    onMutate: () => client.cancelQueries({ queryKey }),
    onSuccess: (result) => client.setQueryData(queryKey, result),
  });
  const state = status.data?.status;
  const busy = check.isPending || state === "checking" || state === "downloading";
  const message = !backendAvailable ? "请在桌面应用中检查更新。"
    : check.isPending ? "正在检查更新…"
    : check.error?.message || status.error?.message || describeUpdate(status.data);

  return <Card>
    <CardHeader>
      <CardTitle>应用更新</CardTitle>
      <CardDescription role="status">{message}</CardDescription>
      <CardAction>
        <Button variant="outline" disabled={!backendAvailable || busy || state === "ready" || state === "unsupported"} onClick={() => check.mutate()}>
          {busy ? <Spinner data-icon="inline-start" /> : <RefreshCwIcon data-icon="inline-start" />}检查更新
        </Button>
      </CardAction>
    </CardHeader>
  </Card>;
}
