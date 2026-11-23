import type { QueryClient } from "@tanstack/react-query";
import type { AgentEvent } from "./types";

const GAME_MUTATIONS = new Set(["register_game", "update_game", "remove_game", "set_game_adapter"]);

function record(value: unknown): Record<string, unknown> | undefined {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : undefined;
}

/** Read successful game mutations from the common desktop and inbound MCP result receipt. */
export function gameMutationsFromResults(results: AgentEvent[]) {
  return results.flatMap((event) => {
    if (event.kind !== "tool-result" || event.payload.status !== "done") return [];
    const name = event.payload.name;
    const result = record(event.payload.result);
    if (typeof name !== "string" || !GAME_MUTATIONS.has(name) || typeof result?.gameId !== "string") return [];
    return [{ name, gameId: result.gameId }];
  });
}

export async function invalidateGameQueries(
  client: QueryClient,
  { gameIds = [], workspaceChanged = false }: { gameIds?: Iterable<string>; workspaceChanged?: boolean } = {},
) {
  const invalidations = [client.invalidateQueries({ queryKey: ["games"] })];
  for (const id of new Set(gameIds)) {
    invalidations.push(client.invalidateQueries({ queryKey: ["game", id] }));
    invalidations.push(client.invalidateQueries({ queryKey: ["mod-library", id] }));
  }
  if (workspaceChanged) {
    invalidations.push(client.invalidateQueries({ queryKey: ["conversations"] }));
    invalidations.push(client.invalidateQueries({ queryKey: ["conversation"] }));
  }
  await Promise.all(invalidations);
}
