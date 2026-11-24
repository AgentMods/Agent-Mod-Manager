import { formatNativeError } from "./native-error";
import { tr } from "@/lib/i18n";
import { invoke, isTauri, type InvokeArgs, type InvokeOptions } from "@tauri-apps/api/core";
import { useQuery } from "@tanstack/react-query";

export const backendAvailable = isTauri();

export { formatNativeError as errorText };

export async function api<T>(command: string, args?: InvokeArgs, options?: InvokeOptions): Promise<T> {
  if (!backendAvailable) throw new Error(tr("当前为浏览器预览，请在桌面应用中连接本地服务。"));
  try { return await invoke<T>(command, args, options); }
  catch (error) { throw new Error(formatNativeError(error), { cause: error }); }
}

export function useBackendQuery<T>(key: unknown[], command: string, args?: Record<string, unknown>, enabled = true) {
  return useQuery<T>({ queryKey: key, queryFn: () => api<T>(command, args), enabled: backendAvailable && enabled, retry: false });
}
