import type { Conversation, Game, GameWorkspace } from "./types";

export function conversationWorkspace(conversation: Conversation): GameWorkspace {
  return conversation.workspace_game_ids !== undefined ? conversation.workspace_game_ids
    : conversation.game_id ? [conversation.game_id] : null;
}

export function workspaceLabel(workspace: GameWorkspace, games: Game[] | undefined): string {
  if (workspace === null) return "全部游戏";
  if (!workspace.length) return "未选择游戏";
  if (workspace.length === 1) return games?.find(game => game.id === workspace[0])?.name ?? "已选 1 款游戏";
  return `已选 ${workspace.length} 款游戏`;
}
