import type { Message } from './types';

// Local source identities used by the native Browse adapter, including its catalog aliases.
const sources = {
  glossmod_http: { name: 'Mod客栈', catalog: null },
  glossmod_3dm: { name: '3DM', catalog: '3dm' },
  nexusmods: { name: 'Nexus Mods', catalog: 'nexus' },
  curseforge: { name: 'CurseForge', catalog: 'curseforge' },
  gamebanana: { name: 'GameBanana', catalog: null },
  modio: { name: 'mod.io', catalog: null },
  thunderstore: { name: 'Thunderstore', catalog: 'thunderstore' },
} as const;

type SourceId = keyof typeof sources;
export interface BrowseInstallRequest {
  kind: 'browse-install';
  gameId: string;
  providerGameId: string;
  sourceId: SourceId;
  sourceName: string;
  projectId: string;
  catalogSourceId?: string | null;
}

// i18n-ignore-start: exact persisted native templates from browse/install/scope.rs.
const prefix = '用户从浏览卡片发起了本机安装。下列 JSON 由原生浏览结果生成，不来自翻译、作品文案或模型推断：';
const scope = '\n只处理这一个来源作品和登记游戏，不要搜索替代品、打开任意 URL 或安装其他依赖。';
const ending = '\n可先用 list_mod_library 检查已有的本作品归档。用 match_mod_install 检查已归档原包，再用 install_library_mod 审批原生安装计划。兼容性、版本、映射或用户意图不明确时用 ask_user；如果无法完成就停下说明，不根据下载成功或模型文本宣称已安装。明确停止于一个经验证的安装事务。';
const legacyStockWorkflow = '用原厂来源的附件或版本列表确认版本，对同一作品调用 download_file 或 download_version。只收到可信下载回执后，用 file_received_mod 归档模型收到的精确文件名；若无法验证、账号拒绝或只能从网站手动下载，明确停止并提示用户到原作品页选择并手动导入。';
const stockWorkflow = '已有可信下载回执时，用 file_received_mod 归档回执中的精确文件名；省略 gameName、modName 和 projectPage。否则用原厂附件或版本列表确认版本，对同一作品调用 download_file 或 download_version。若无法验证、账号拒绝或只能从网站手动下载，明确停止并提示用户到原作品页选择并手动导入。下载工具直接返回 artifactId 时，使用该已归档原包。';

function workflows(source: SourceId, legacy: boolean) {
  if (source === 'glossmod_http') return ['用官方 Mod 客栈 list_work_files 查询此 work_id，核对附件、版本与 entitlement；只使用官方 get_work_file_download 下载本作品的附件。'];
  if (source === 'glossmod_3dm') return legacy
    ? ['优先用原厂 3DM MCP get-mod-detail 核对作品资源；只使用返回的会话 resolution_id 调用 download_mod，不传 URL。若不能验证官方直链，提示用户在原作品页选择文件并手动导入。']
    : ['用原厂 3DM MCP get-mod-detail 查询作品，id 取 projectId 的整数值。用返回的 download.resolution_id 调用 download_mod。若不能验证官方直链，提示用户在原作品页选择文件并手动导入。'];
  return legacy ? [legacyStockWorkflow] : [stockWorkflow, legacyStockWorkflow];
}
// i18n-ignore-end

function isSourceId(value: unknown): value is SourceId {
  return typeof value === 'string' && Object.hasOwn(sources, value);
}

// Called for the opening user message in an install conversation. Browse and free text
// both persist kind=user, so later turns, edited origins and partial matches stay literal.
export function parseBrowseInstallRequest({ role, origin, content, attachments }: Message): BrowseInstallRequest | null {
  if (role !== 'user' || (origin != null && origin.kind !== 'user') || attachments?.length || !content.startsWith(prefix)) return null;
  const lineEnd = content.indexOf('\n', prefix.length);
  if (lineEnd < 0) return null;
  const raw = content.slice(prefix.length, lineEnd);
  let identity: unknown;
  try { identity = JSON.parse(raw); } catch { return null; }
  if (!identity || typeof identity !== 'object' || Array.isArray(identity)) return null;
  const { gameId, projectId, providerGameId, sourceId, catalogSourceId } = identity as Record<string, unknown>;
  if (!isSourceId(sourceId) || typeof gameId !== 'string' || !gameId.trim() ||
    typeof projectId !== 'string' || !projectId.trim() || typeof providerGameId !== 'string' || !providerGameId.trim()) return null;
  const legacy = !Object.hasOwn(identity, 'catalogSourceId');
  const source = sources[sourceId];
  if (!legacy && catalogSourceId !== source.catalog) return null;
  // serde_json emits sorted keys. Rebuild both released identity shapes to reject
  // extra/duplicate fields, alternate encodings and instructions appended by a user.
  const ids = { gameId, projectId, providerGameId, sourceId };
  if (raw !== JSON.stringify(legacy ? ids : { catalogSourceId, ...ids })) return null;
  const instructions = content.slice(lineEnd);
  if (!workflows(sourceId, legacy).some((workflow) => instructions === scope + workflow + ending)) return null;
  return { kind: 'browse-install', ...ids, sourceName: source.name, catalogSourceId: legacy ? undefined : source.catalog };
}
