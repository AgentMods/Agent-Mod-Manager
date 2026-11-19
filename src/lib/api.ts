import { invoke, isTauri } from "@tauri-apps/api/core";
import { useQuery } from "@tanstack/react-query";

export const backendAvailable = isTauri();

export function errorText(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return error == null ? "未知错误" : JSON.stringify(error);
}

export async function api<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!backendAvailable) throw new Error("当前为浏览器预览，请在桌面应用中连接本地服务。");
  try { return await invoke<T>(command, args); }
  catch (error) { throw new Error(errorText(error)); }
}

export function useBackendQuery<T>(key: unknown[], command: string, args?: Record<string, unknown>, enabled = true) {
  return useQuery<T>({ queryKey: key, queryFn: () => api<T>(command, args), enabled: backendAvailable && enabled, retry: false });
}
