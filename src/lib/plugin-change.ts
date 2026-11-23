import { useMutation, useMutationState, useQueryClient } from "@tanstack/react-query";
import { api } from "./api";
import { useUI } from "./store";
import type { Plugin } from "./types";

type PluginChange = { action: "install"; slug: string; name: string; pluginId: string; releaseId: string }
  | { action: "remove"; slug: string; name: string };
export const pluginChangeKey = ["change-installed-plugin"];

// The catalog (including the game picker) and the installed list share one write in flight,
// even if a user switches tabs while the native operation is running.
export function usePluginChange() {
  const client = useQueryClient();
  const notify = useUI(s => s.notify);
  const pending = useMutationState({ filters: { mutationKey: pluginChangeKey, status: "pending" }, select: m => m.state.variables as PluginChange | { action: "register-game" } })[0];
  const mutation = useMutation({
    mutationKey: pluginChangeKey,
    networkMode: "always", // removal is a local desktop operation, also available offline
    mutationFn: (change: PluginChange): Promise<void | { transactionId: string }> => change.action === "install"
      ? api<void>("install_manager_plugin", { pluginId: change.pluginId, releaseId: change.releaseId })
      : api<{ transactionId: string }>("remove_installed_plugin", { slug: change.slug }),
    onSuccess: async (_, change) => {
      if (change.action === "remove") {
        // The native commit is definitive even if React Query defers an offline refetch.
        client.setQueryData<Plugin[]>(["plugins"], current => current?.filter(plugin => plugin.slug !== change.slug));
      }
      const keys = ["plugins", "skills", "mcp"];
      await Promise.all(keys.map(key => client.invalidateQueries({ queryKey: [key] })));
      notify(`${change.action === "install" ? "已安装" : "已移除"}「${change.name}」`);
    },
  });
  function run(change: PluginChange, afterSuccess?: () => void) {
    if (!client.isMutating({ mutationKey: pluginChangeKey })) mutation.mutate(change, { onSuccess: afterSuccess });
  }
  return { run, pending, error: mutation.error, failed: mutation.isError ? mutation.variables : undefined };
}
