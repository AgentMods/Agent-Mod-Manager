import type { QueryClient } from "@tanstack/react-query";
import { dashboardStatsKey } from "./dashboard-stats";
import type { AgentEvent } from "./types";

function record(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" ? value as Record<string, unknown> : {};
}

function id(value: unknown) {
  return typeof value === "string" && value.length ? value : null;
}

// Some receipts deliberately omit the game. The matching tool-start holds the
// exact request arguments; never infer a scope from the visible game or a name.
export function invalidateAgentModQueries(client: QueryClient, events: AgentEvent[]) {
  const libraryGames = new Set<string>();
  const installGames = new Set<string>();
  let allLibraryGames = false;
  let allInstallGames = false;
  const rememberLibraryReceipt = (receipt: Record<string, unknown>) => {
    const gameId = id(receipt.gameId);
    if (gameId) libraryGames.add(gameId);
    else if (receipt.gameId === null) allLibraryGames = true;
  };
  const starts = new Map(events.filter(event => event.kind === "tool-start" && id(event.payload.toolCallId))
    .map(event => [event.payload.toolCallId, event]));
  const approvals = new Map(events.filter(event => event.kind === "approval-request" && id(event.payload.toolCallId))
    .map(event => [event.payload.toolCallId, event.payload]));
  for (const event of events) {
    if (event.kind !== "tool-result" || event.payload.status !== "done") continue;
    const start = starts.get(event.payload.toolCallId);
    const args = record(start && start.payload.name === event.payload.name ? start.payload.arguments : undefined);
    const receipt = record(event.payload.result);
    switch (event.payload.name) {
      case "bind_mod_library_entry":
        // Binding removes an unbound entry from other same-name game lists.
        allLibraryGames = true;
        break;
      case "download_mod":
      case "download_web_mod":
      case "retry_mod_library_download":
      case "file_received_mod":
        rememberLibraryReceipt(receipt);
        break;
      case "install_library_mod":
      case "install_mod":
      case "forget_uninstalled_mod":
      case "uninstall_mod": {
        const gameId = id(record(args.request).gameId) ?? id(args.game_id) ?? id(receipt.gameId);
        if (gameId) installGames.add(gameId);
        else allInstallGames = true; // Legacy installs/uninstalls may not report a game.
        break;
      }
      case "rollback_transaction": {
        const gameId = id(args.game_id);
        if (gameId && id(args.install_id) && id(args.transaction_id)) installGames.add(gameId);
        break;
      }
      default: {
        // Trust host implementation provenance, retaining native approval
        // previews for events recorded before the host marker existed.
        const name = event.payload.name;
        const approval = approvals.get(event.payload.toolCallId);
        if (typeof name !== "string" || !name.startsWith("mcp__")) break;
        const preview = record(approval?.preview);
        const artifact = record(receipt.artifact);
        const trusted = event.payload.hostToolKind === "glossmod_attachment_download" ||
          (approval?.title === name && preview.tool === "get_work_file_download" && preview.destination === "Mods/");
        if (trusted &&
          receipt.cataloged === true && id(receipt.artifactId) && artifact.artifactId === receipt.artifactId) {
          rememberLibraryReceipt(artifact);
        }
      }
    }
  }
  const invalidate = (...queryKey: string[]) => { void client.invalidateQueries({ queryKey }); };
  if (allInstallGames || installGames.size) {
    invalidate("games");
    invalidate("trash");
    if (allInstallGames) {
      invalidate("installs");
      invalidate("mod-tags");
      invalidate("game");
      allLibraryGames = true;
    } else {
      for (const gameId of installGames) {
        invalidate("installs", gameId);
        invalidate("mod-tags", gameId);
        invalidate("game", gameId);
        libraryGames.add(gameId);
      }
    }
  }
  if (allLibraryGames) invalidate("mod-library");
  else for (const gameId of libraryGames) invalidate("mod-library", gameId);
  if (allLibraryGames || libraryGames.size) invalidate(...dashboardStatsKey);
}
