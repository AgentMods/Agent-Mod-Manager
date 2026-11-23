import type { Game, GameCandidate } from "./types";
import { gameDirectoryKey } from "./paths";

/** Keep the scan's first spelling/ranking, but don't lose a backend registration on an alias row. */
export function reconcileCandidates(candidates: GameCandidate[], games: Game[] = []): GameCandidate[] {
  const registered = new Map(games.map((game) => [gameDirectoryKey(game.rootPath), game]));
  const distinct = new Map<string, GameCandidate>();
  for (const candidate of candidates) {
    const key = gameDirectoryKey(candidate.rootPath);
    const previous = distinct.get(key);
    if (!previous || (!previous.registeredGameId && candidate.registeredGameId)) {
      distinct.set(key, { ...(previous ?? candidate), registeredGameId: candidate.registeredGameId });
    }
  }
  return Array.from(distinct, ([key, candidate]) => {
    const game = registered.get(key);
    if (game) return {
      ...candidate, registeredGameId: game.id, suggestedAdapterSlug: game.adapterSlug,
      confidence: game.confidence, status: game.adapterSlug ? "high_confidence" : "unidentified",
    };
    return candidate;
  });
}
