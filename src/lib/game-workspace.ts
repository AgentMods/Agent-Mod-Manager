import { tr } from "@/lib/i18n";
import type { Conversation, Game, GameWorkspace } from "./types";

export function conversationWorkspace(conversation: Conversation): GameWorkspace {
  return conversation.workspace_game_ids !== undefined ? conversation.workspace_game_ids
    : conversation.game_id ? [conversation.game_id] : null;
}

export function workspaceLabel(workspace: GameWorkspace, games: Game[] | undefined): string {
  if (workspace === null) return tr("全部游戏");
  if (!workspace.length) return tr("未选择游戏");
  const game = workspace.length === 1 ? games?.find(game => game.id === workspace[0]) : undefined;
  if (game) return game.name;
  return tr("已选 {length} 款游戏", { length: workspace.length });
}
