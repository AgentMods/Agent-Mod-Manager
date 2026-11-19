export type PageName = "library" | "detail" | "agent" | "discover" | "plugins" | "trash" | "history" | "settings";
export type PermissionMode = "request_approval" | "auto_approve" | "full_trust";
export interface Game {
  id: string; name: string; slug: string; rootPath: string; platform: string;
  adapterSlug: string | null; adapterVersion: string | null; confidence: number;
  coverPath: string | null; steamAppId: string | null; installedModCount: number;
  enabledModCount: number; metadata: Record<string, unknown>; createdAt: string; updatedAt: string;
}
export interface Conversation {
  id: string; title: string; game_id: string | null; task_summary: string | null;
  archived: boolean; pinned: boolean; message_count: number; created_at: string;
  updated_at: string; permission_mode: PermissionMode | null;
}
export interface Message {
  id: string; role: string; content: string; reasoning: string | null;
  tool_calls: unknown; tool_call_id: string | null; token_count?: number; created_at?: string;
}
export interface AgentEvent {
  seq: number; turnId: string; conversationId: string; messageId: string | null;
  kind: string; payload: Record<string, unknown>; createdAt: string;
}
export interface FilePlan {
  id: string; summary: string; total_bytes: number; created_at: string; requires_review: boolean;
  entries: Array<{operation: {op: string; src?: string; dst: string}; impact: string;
    before: {kind: string; size: number; sha256: string} | null;
    source: {kind: string; size: number; sha256: string} | null}>;
}
export interface AppSettings {
  base_url: string; model: string; context_budget: number; permission_mode: PermissionMode; api_key_ref: string | null;
}
export interface Plugin {
  slug: string; name: string; gameName: string; version: string; description: string;
  source: string; enabled: boolean; validated: boolean; installPath: string;
}
export interface TrashItem {
  id: string; original_path: string; trash_path: string; transaction_id: string | null;
  size: number; is_dir: boolean; game_id: string | null; label: string; reason: string;
  created_at: string; restored_at: string | null; exists: boolean; restore_requires_rollback: boolean;
}
