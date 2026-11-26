import { useEffect, useEffectEvent, useId, useRef, useState } from "react";
import {
  ChevronDownIcon,
  ChevronLeftIcon,
  ChevronRightIcon,
  CompassIcon,
  Gamepad2Icon,
  ListFilterIcon,
  ListTodoIcon,
  SearchIcon,
} from "lucide-react";
import { useQueryClient } from "@tanstack/react-query";
import {
  browseInstallStatus, browseInstallStatusLabel, startBrowseInstall, usePendingBrowseInstall, type BrowseInstallIndexItem,
} from "@/lib/browse-install";
import { api, backendAvailable, useBackendQuery } from "@/lib/api";
import { tr } from "@/lib/i18n";
import { useUI } from "@/lib/store";
import type { Game } from "@/lib/types";
import {
  browseTypes,
  resolveBrowseType,
  saveBrowseGames,
  storedBrowseGames,
  useBrowseSearch,
  type BrowseGrant,
  type BrowseItem,
} from "@/lib/browse";
import { BrowseGamePicker } from "@/components/BrowseGamePicker";
import { BrowseModCard } from "@/components/BrowseModCard";
import { useBrowseSources } from "@/lib/browse-sources";
import { BrowseSourcePicker } from "@/components/BrowseSourcePicker";
import { BrowseNotices } from "@/components/BrowseNotices";
import { BrowseTaskList } from "@/components/BrowseTaskList";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { NativeSelect } from "@/components/ui/native-select";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";
import { Spinner } from "@/components/ui/spinner";
import { EmptyState, ErrorState, Page, PageHeader } from "@/components/ui";

const PAGE_SIZE = 24;
const RESUME_TTL_MS = 5 * 60_000;

// Window-local view state. The native search rechecks the grant before serving any catalog data.
interface BrowseVisit {
  grant: BrowseGrant;
  gamesKey: string;
  source: string;
  text: string;
  type: string;
  tag: string;
  page: number;
  lastIndex: number;
  leftAt: number;
}
let lastVisit: BrowseVisit | null = null;

export default function BrowsePage() {
  const games = useBackendQuery<Game[]>(["games"], "list_games");
  const navigate = useUI((s) => s.navigate);
  const notify = useUI((s) => s.notify);
  const [saved, setSaved] = useState(storedBrowseGames);
  const gameIds = (games.data ?? [])
    .filter((g) => (saved === null ? games.data?.length === 1 : saved.includes(g.id)))
    .map((g) => g.id);
  useEffect(() => {
    if (saved === null && games.data?.length === 1) {
      const ids = [games.data[0].id];
      saveBrowseGames(ids);
      setSaved(ids);
    }
  }, [games.data, saved]);
  const [picker, setPicker] = useState(false);
  const [visit] = useState(() =>
    lastVisit && Date.now() - lastVisit.leftAt < RESUME_TTL_MS ? lastVisit : null,
  );
  const [source, setSource] = useState(visit?.source ?? "");
  const [draft, setDraft] = useState(visit?.text ?? "");
  const [text, setText] = useState(visit?.text ?? "");
  const [type, setType] = useState(visit?.type ?? "");
  const [tag, setTag] = useState(visit?.tag ?? "");
  const [showFilters, setShowFilters] = useState(false);
  const [revision, setRevision] = useState(0);
  const [error, setError] = useState<unknown>(null);
  const [page, setPage] = useState(visit?.page ?? 0);
  const restoring = useRef(visit);
  const [taskList, setTaskList] = useState(false);
  const client = useQueryClient();
  const pendingInstall = usePendingBrowseInstall();
  const pendingInstallLabelId = useId();
  const scope = useBrowseSources(visit?.grant ?? null, gameIds.length > 0, (next) => {
    newQuery();
    setSource((current) => next.sources.some((s) => s.id === current) ? current : "");
  });
  const { sources, grant } = scope;
  const search = useBrowseSearch(grant, gameIds, source, text, revision);
  const gamesKey = JSON.stringify(gameIds);
  const rememberVisit = useEffectEvent(() => {
    lastVisit = grant && !scope.busy && !search.error
      ? { grant, gamesKey, source, text, type, tag, page, lastIndex: search.lastIndex, leftAt: Date.now() }
      : null;
  });
  useEffect(() => () => rememberVisit(), []);
  useEffect(() => {
    const prior = restoring.current;
    if (!prior || !search.id) return;
    if (gamesKey !== prior.gamesKey) {
      restoring.current = null;
      setPage(0);
      return;
    }
    const target = prior.type || prior.tag ? prior.lastIndex : prior.page;
    if (search.lastIndex >= target || search.error || (!search.busy && !search.hasNext)) {
      restoring.current = null;
    } else if (!search.busy && search.hasNext) {
      search.loadNext();
    }
  }, [gamesKey, search]);
  const sourceName = (id: string) => sources.data?.find((s) => s.id === id)?.name ?? id;
  const gameName = (id: string) => games.data?.find((g) => g.id === id)?.name ?? id;
  const all = [
    ...new Map(search.ready.flatMap((frame) => frame.items.map((item) => [item.id, item] as const))).values(),
  ];
  const tags = new Map(all.flatMap((item) => item.tags.map((t) => [t.key, t.label] as const)));
  const categoryNames = browseTypes();
  const filtered = !!type || !!tag;
  const matches = all.filter(
    (item) =>
      (!type || resolveBrowseType(item.typeKey, categoryNames) === type) &&
      (!tag || item.tags.some((t) => t.key === tag)),
  );
  const shownPage = filtered
    ? Math.min(page, Math.max(0, Math.ceil(matches.length / PAGE_SIZE) - 1))
    : Math.min(page, search.lastIndex);
  const visible = filtered
    ? matches.slice(shownPage * PAGE_SIZE, (shownPage + 1) * PAGE_SIZE)
    : (search.ready.find((p) => p.page === shownPage)?.items ?? []);
  const latest = search.ready.at(-1);
  const hasLocalNext = filtered ? (shownPage + 1) * PAGE_SIZE < matches.length : shownPage < search.lastIndex;
  const hasNext = hasLocalNext || search.hasNext;
  const fillCurrentPage = filtered && !hasLocalNext && visible.length < PAGE_SIZE;
  const canBrowse = !!grant && scope.available && gameIds.length > 0;
  const reset = () => {
    restoring.current = null;
    setPage(0);
    setError(null);
  };
  function newQuery() {
    reset();
    setTag("");
    setType("");
  }
  async function open(item: BrowseItem) {
    if (!item.canOpenProject || !search.id) return;
    try {
      await api("browse_open_project", { searchId: search.id, itemId: item.id });
    } catch (e) {
      setError(e);
    }
  }
  async function startInstall(target: BrowseInstallIndexItem) {
    if (target.searchId !== search.id || search.error) return;
    return startBrowseInstall(client, target, target.gameId);
  }
  async function installFromCard(item: BrowseItem, gameId: string) {
    if (!search.id || !item.gameIds.includes(gameId)) return;
    const task = await startInstall({ title: item.title, searchId: search.id, itemId: item.id, gameId });
    if (!task) return;
    const status = browseInstallStatus(task);
    notify(status === "running" ? tr("安装任务已在后台启动，可在「安装任务」中查看。") : browseInstallStatusLabel(status));
  }
  function next() {
    restoring.current = null;
    if (hasLocalNext) setPage(shownPage + 1);
    else if (search.hasNext) {
      search.loadNext();
      if (!fillCurrentPage) setPage(shownPage + 1);
    }
  }
  function applySources(ids: string[]) {
    const sameSources = grant?.sources.length === ids.length && grant.sources.every((s) => ids.includes(s.id));
    if (!sameSources || scope.error || search.error) scope.apply(ids);
  }
  function retrySearch() {
    reset();
    const cause = search.error instanceof Error ? search.error.cause : null;
    if (cause && typeof cause === "object" && "key" in cause && cause.key === "browse.authorization_changed") {
      scope.retry();
    } else {
      setRevision((r) => r + 1);
    }
  }
  return (
    <Page>
      <PageHeader
        title={tr("游览")}
        subtitle={tr("在已连接的 Mod 来源中，为你的游戏寻找新作品。")}
        actions={
          <div className="flex flex-wrap items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              aria-describedby={pendingInstall ? pendingInstallLabelId : undefined}
              onClick={() => setTaskList(true)}
              disabled={!backendAvailable}
            >
              <ListTodoIcon />
              {tr("安装任务")}
            </Button>
            {pendingInstall && <span id={pendingInstallLabelId} role="status">
              <Badge variant="warning">{browseInstallStatusLabel("running", pendingInstall)}</Badge>
            </span>}
          </div>
        }
      />
      {backendAvailable && !!games.data?.length && (
        <section aria-label={tr("游览筛选")} className="flex flex-col gap-2 rounded-xl border bg-card p-3">
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <form
              className="flex min-w-0 flex-[1_1_18rem] items-center gap-2"
              onSubmit={(e) => {
                e.preventDefault();
                if (!canBrowse || scope.busy) return;
                const value = draft.trim();
                if (text !== value) newQuery();
                else {
                  reset();
                  setRevision((r) => r + 1);
                }
                setText(value);
              }}
            >
              <InputGroup className="min-w-0">
                <InputGroupInput
                  type="search"
                  aria-label={tr("搜索 Mod")}
                  placeholder={tr("搜索 Mod")}
                  value={draft}
                  onChange={(e) => {
                    const v = e.target.value;
                    if (new TextEncoder().encode(v).length <= 240) setDraft(v);
                  }}
                />
                <InputGroupAddon>
                  <SearchIcon aria-hidden="true" />
                </InputGroupAddon>
              </InputGroup>
              <Button type="submit" size="sm" disabled={!canBrowse || scope.busy}>
                {tr("搜索")}
              </Button>
            </form>
            <Button
              variant="outline"
              size="sm"
              className="max-w-full"
              aria-label={tr("选择游览的游戏")}
              onClick={() => setPicker(true)}
            >
              <Gamepad2Icon />
              <span className="min-w-0 truncate">
                {gameIds.length === 1 ? gameName(gameIds[0]) : tr("游戏（{count}）", { count: gameIds.length })}
              </span>
              <ChevronDownIcon />
            </Button>
            <Button
              className="sm:hidden"
              variant="outline"
              size="sm"
              aria-expanded={showFilters}
              aria-controls="browse-filters"
              onClick={() => setShowFilters(!showFilters)}
            >
              <ListFilterIcon />
              {tr("筛选")}
            </Button>
          </div>
          {!!gameIds.length && !!sources.data?.length && (
            <div
              id="browse-filters"
              className={`${showFilters ? "flex" : "hidden"} min-w-0 flex-wrap items-center gap-2 sm:flex`}
            >
              <NativeSelect
                size="sm"
                disabled={!canBrowse || scope.busy}
                aria-label={tr("来源")}
                value={source}
                onChange={(e) => {
                  newQuery();
                  setSource(e.target.value);
                }}
              >
                <option value="">{tr("全部来源")}</option>
                {grant?.sources.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.name}
                  </option>
                ))}
              </NativeSelect>
              <NativeSelect
                size="sm"
                disabled={!canBrowse || scope.busy}
                aria-label={tr("类型")}
                value={type}
                onChange={(e) => {
                  reset();
                  setType(e.target.value);
                }}
              >
                <option value="">{tr("全部类型")}</option>
                {Object.entries(categoryNames).map(([key, name]) => (
                  <option key={key} value={key}>
                    {name}
                  </option>
                ))}
              </NativeSelect>
              <NativeSelect
                size="sm"
                disabled={!canBrowse || scope.busy}
                aria-label={tr("标签")}
                value={tag}
                onChange={(e) => {
                  reset();
                  setTag(e.target.value);
                }}
              >
                <option value="">{tr("全部标签")}</option>
                {tag && !tags.has(tag) && <option value={tag}>{tag}</option>}
                {Array.from(tags).map(([key, label]) => (
                  <option key={key} value={key}>
                    {label}
                  </option>
                ))}
              </NativeSelect>
              <BrowseSourcePicker
                sources={sources.data ?? []}
                selected={grant?.sources ?? sources.data?.filter((s) => s.status === "available") ?? []}
                busy={scope.busy}
                refresh={() => void sources.refetch({ cancelRefetch: false })}
                apply={applySources}
              />
            </div>
          )}
        </section>
      )}
      <ErrorState error={scope.error || error || games.error || sources.error || search.error} />
      {!backendAvailable ? (
        <EmptyState title={tr("当前为浏览器预览，请在桌面应用中连接本地服务。")} icon={<CompassIcon />} />
      ) : games.isPending || sources.isPending ? (
        <div role="status" className="flex items-center gap-2 text-sm">
          <Spinner />
          {tr("正在加载...")}
        </div>
      ) : games.isError || (sources.isError && !sources.data) ? (
        <Button
          variant="outline"
          onClick={() => {
            void games.refetch();
            void sources.refetch();
          }}
        >
          {tr("重试")}
        </Button>
      ) : !games.data?.length ? (
        <EmptyState
          title={tr("游戏库中还没有游戏。")}
          action={<Button onClick={() => navigate("library")}>{tr("前往游戏库")}</Button>}
        />
      ) : !gameIds.length ? (
        <EmptyState
          icon={<Gamepad2Icon />}
          title={tr("先选游戏，再开始游览")}
          action={<Button onClick={() => setPicker(true)}>{tr("选择游戏")}</Button>}
        />
      ) : !scope.available ? (
        <EmptyState icon={<CompassIcon />} title={tr("没有可用的 Mod 来源")} action={
          <div className="flex gap-2">
            <Button variant="outline" onClick={() => navigate("settings")}>{tr("设置")}</Button>
            <Button variant="ghost" disabled={sources.isFetching} onClick={() => void sources.refetch()}>{tr("刷新")}</Button>
          </div>
        }>{tr("请在「设置 → 工具」连接 Mod 来源，然后刷新。")}</EmptyState>
      ) : !grant && !scope.error ? (
        <div role="status" className="flex items-center gap-2 text-sm"><Spinner />{tr("正在加载...")}</div>
      ) : (
        <>
          {!!scope.error && <Button variant="outline" onClick={scope.retry}>{tr("重试")}</Button>}
          <BrowseNotices
            issues={latest?.issues ?? []}
            choices={latest?.gameChoices ?? []}
            gameName={gameName}
            sourceName={sourceName}
            select={async (c, id) => {
              try {
                await api("browse_select_game_match", {
                  searchId: search.id,
                  gameId: c.gameId,
                  sourceId: c.sourceId,
                  providerGameId: id,
                });
                reset();
                setRevision((v) => v + 1);
              } catch (e) {
                setError(e);
              }
            }}
          />
          {!!search.error && !scope.error && (
            <Button variant="outline" disabled={scope.busy} onClick={retrySearch}>{tr("重试")}</Button>
          )}
          {filtered && (
            <p className="text-xs text-muted-foreground">
              {tr("类型和标签只筛选已获取的作品。继续翻页可查找更多匹配。")}
            </p>
          )}
          {search.enriching && (
            <span role="status" className="flex items-center gap-2 text-xs text-muted-foreground">
              <Spinner />
              {tr("Agent 正在整理作品资料…")}
            </span>
          )}
          {search.busy && (
            <div role="status" className="flex items-center gap-2 text-sm text-muted-foreground">
              <Spinner />
              {tr("正在查询 Mod 目录…")}
            </div>
          )}
          {!!visible.length && (
            <div className="grid min-w-0 grid-cols-[repeat(auto-fill,minmax(min(100%,17rem),1fr))] items-stretch gap-4">
              {visible.map((item) => (
                <BrowseModCard
                  key={JSON.stringify([search.id, item.id])}
                  searchId={search.id}
                  item={item}
                  source={sourceName(item.sourceId)}
                  gameName={gameName}
                  onOpen={() => void open(item)}
                  canInstall={!!search.id && !search.error}
                  onInstall={(gameId) => installFromCard(item, gameId)}
                />
              ))}
            </div>
          )}
          {grant && !visible.length && !search.busy && !search.error && (
            <EmptyState
              icon={<CompassIcon />}
              title={
                filtered
                  ? tr("已获取的作品中没有匹配项")
                  : latest?.issues.length || latest?.gameChoices.length
                    ? tr("暂无可展示的作品")
                    : tr("没有找到作品")
              }
            />
          )}
          {(!!latest || shownPage > 0) && (
            <nav
              aria-label={tr("作品分页")}
              className="flex flex-wrap items-center justify-center gap-3 border-t pt-4"
            >
              <Button variant="outline" size="sm" disabled={!shownPage} onClick={() => {
                restoring.current = null;
                setPage(shownPage - 1);
              }}>
                <ChevronLeftIcon />
                {tr("上一页")}
              </Button>
              <span className="text-sm tabular-nums">{tr("第 {page} 页", { page: shownPage + 1 })}</span>
              <Button
                size="sm"
                variant="outline"
                disabled={!hasNext || (search.busy && !hasLocalNext)}
                onClick={next}
              >
                {fillCurrentPage && hasNext ? tr("查找更多匹配") : tr("下一页")}
                <ChevronRightIcon />
              </Button>
            </nav>
          )}
        </>
      )}
      {picker && (
        <BrowseGamePicker
          games={games.data ?? []}
          selected={gameIds}
          close={() => setPicker(false)}
          apply={(ids) => {
            saveBrowseGames(ids);
            setSaved(ids);
            newQuery();
            setPicker(false);
          }}
        />
      )}
      {taskList && (
        <BrowseTaskList
          gameName={gameName}
          close={() => setTaskList(false)}
          retrySearchId={search.error ? null : search.id}
          retry={startInstall}
        />
      )}
    </Page>
  );
}
