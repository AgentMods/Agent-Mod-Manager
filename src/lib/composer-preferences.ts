import { queryOptions, type QueryClient } from "@tanstack/react-query";
import { api } from "./api";
import type { ComposerPreferences, PermissionMode } from "./types";

export const composerPreferencesQuery = queryOptions({
  queryKey: ["composer-preferences"],
  queryFn: () => api<ComposerPreferences>("get_composer_preferences"),
});

// An explicit empty selection means “use conversation settings”, even when a previous
// turn remembered Full Trust. Only an untouched selector restores the remembered value.
export function turnPermission(selection: PermissionMode | "" | undefined, saved?: ComposerPreferences) {
  return (selection ?? saved?.permission_mode) || null;
}

export function rememberComposerPreferences(client: QueryClient, preferences: ComposerPreferences) {
  client.setQueryData(composerPreferencesQuery.queryKey, preferences);
  // The turn has already started; a preference write must not make it look like a failed start.
  void api("set_composer_preferences", { preferences }).catch(() => undefined);
}

// Explicit selections do not depend on saved preferences. Return the complete cached
// record separately so callers changing only permission never erase unknown settings.
export async function resolveComposerPermission(client: QueryClient, selection: PermissionMode | "" | undefined) {
  const preferences = selection === undefined
    ? await client.ensureQueryData(composerPreferencesQuery)
    : client.getQueryData(composerPreferencesQuery.queryKey);
  return { permissionMode: turnPermission(selection, preferences), preferences };
}