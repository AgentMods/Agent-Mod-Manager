export type PageName = "library" | "detail" | "agent" | "plugins" | "trash" | "history" | "settings" | "onboarding";
export type PermissionMode = "request_approval" | "auto_approve" | "full_trust";
export interface Game {
  id: string; name: string; slug: string; rootPath: string; platform: string;
  adapterSlug: string | null; adapterVersion: string | null; confidence: number;
  coverPath: string | null; steamAppId: string | null; installedModCount: number;
  enabledModCount: number; metadata: Record<string, unknown>; createdAt: string; updatedAt: string;
}
export type ConversationCategory = "install" | "uninstall" | "remove" | "adapter";
/** null follows all registered games; an empty list deliberately selects none. */
export type GameWorkspace = string[] | null;
export interface Conversation {
  workspace_game_ids?: GameWorkspace;
  category: ConversationCategory | null;
  id: string; title: string; game_id: string | null; task_summary: string | null;
  archived: boolean; pinned: boolean; message_count: number; created_at: string;
  updated_at: string; permission_mode: PermissionMode | null;
  /** Durable checkpoint attached by list_conversations, not create/get_conversation. */
  agent_status?: {
    turn_id: string | null;
    turn_status: string | null;
    event_seq: number;
    pending: { approval_id: string; status: "waiting-answer" | "waiting-approval" } | null;
  };
}
export type AgentAttachment =
  | { kind: "image"; name: string; dataUrl: string }
  | { kind: "document"; name: string; markdown: string };
export interface Message {
  id: string; role: string; content: string; reasoning: string | null;
  attachments?: AgentAttachment[];
  tool_calls: unknown; tool_call_id: string | null; token_count?: number; created_at?: string;
}
export interface AgentEvent {
  seq: number; turnId: string; conversationId: string; messageId: string | null;
  kind: string; payload: Record<string, unknown>; createdAt: string;
}
export type FileOperation =
  | { op: "copy" | "move" | "compress" | "extract"; src: string; dst: string }
  | { op: "mkdir"; src?: never; dst: string }
  | { op: "write_text"; src?: never; dst: string; content: string };
export interface FilePlan {
  id: string; summary: string; total_bytes: number; created_at: string; requires_review: boolean;
  entries: Array<{operation: FileOperation; impact: string;
    before: {kind: string; size: number; sha256: string} | null;
    source: {kind: string; size: number; sha256: string} | null}>;
}
export type GamePlanConflictKind = "file_plan" | "approval" | "recovery";
export interface GamePlanConflict {
  id: string;
  kind: GamePlanConflictKind;
  status: string;
  title: string;
  summary: string;
  paths: string[];
  detail: string | null;
  canResolve: boolean;
  resolutionHint: string | null;
  planId?: string | null;
}
export type ReasoningEffort = "none" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | "ultra";
export interface AgentModelSettings {
  /** The label shown in model pickers and conversation chrome. */
  display_name: string;
  /** The provider-facing model identifier sent with each request. */
  id: string;
  reasoning_levels: ReasoningEffort[];
}
export interface AppSettings {
  models?: AgentModelSettings[];
  base_url: string; model: string; permission_mode: PermissionMode; api_key_ref: string | null; onboarding_completed: boolean;
}
/** Last-used per-turn choices, remembered across restarts and reinstalls. `null` = follow the defaults. */
export interface ComposerPreferences {
  model: string | null; reasoning_effort: ReasoningEffort | null; permission_mode: PermissionMode | null;
}
export interface BootstrapState {
  dataRoot: string; storageMigrationError?: string | null; recoveredTransactions: string[]; recoveryError: string | null; onboardingCompleted: boolean; isFirstRun: boolean; resumeOnboarding: boolean;
}
export interface CatalogPluginRelease {
  id: string; releaseId: string; name: string; gameName?: string | null; slug: string; version: string;
  authorName: string; platforms: string[]; steamAppId: string; description: string; tags?: string[];
}
export interface CatalogGame {
  id: string; name: string; slug: string; aliases: string[]; steamAppId: string; plugins: CatalogPluginRelease[];
}
export interface CatalogSearch { totalGames: number; skippedEntries: number; games: CatalogGame[] }
export interface GameCandidate {
  sources?: { kind: string; steamAppId?: string | null; completeInstall?: boolean | null }[];
  status: "high_confidence" | "candidate" | "unidentified";
  rootPath: string; name: string; confidence: number; suggestedAdapterSlug: string | null; registeredGameId: string | null; platform: string; warnings: string[];
}
export interface ScanReport {
  candidates: GameCandidate[]; warnings: {path: string; message: string}[]; directoriesChecked: number; entriesChecked: number; truncated: boolean; nextCursor?: string | null;
}
export interface Plugin {
  updatedAt: string | null;
  slug: string; name: string; gameName: string; version: string; description: string;
  source: string; hasCloudEvidence: boolean; enabled: boolean; validated: boolean; installPath: string; coverUrl: string | null; mcpServerNames?: string[];
}
export interface TrashItem {
  id: string; original_path: string; trash_path: string; transaction_id: string | null;
  size: number; is_dir: boolean; game_id: string | null; label: string; reason: string;
  created_at: string; restored_at: string | null; exists: boolean; restore_requires_rollback: boolean;
}
