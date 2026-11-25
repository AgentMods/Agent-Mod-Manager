import { useEffect, useState } from "react";
import {
  ChevronDownIcon,
  ChevronLeftIcon,
  ChevronRightIcon,
  CompassIcon,
  Gamepad2Icon,
  ListFilterIcon,
  ListTodoIcon,
  SearchIcon,
  ShieldCheckIcon,
} from "lucide-react";
import { api, backendAvailable, useBackendQuery } from "@/lib/api";
import { tr } from "@/lib/i18n";
import { useUI } from "@/lib/store";
import type { Game } from "@/lib/types";
import {
  browseTypes,
  saveBrowseGames,
  storedBrowseGames,
  useBrowseSearch,
  type BrowseGrant,
  type BrowseItem,
  type BrowseSource,
} from "@/lib/browse";
import { BrowseGamePicker } from "@/components/BrowseGamePicker";
import { BrowseModCard } from "@/components/BrowseModCard";
import { BrowseNotices, BrowseSourceGate } from "@/components/BrowseNotices";
import { BrowseInstallPanel } from "@/components/BrowseInstallPanel";
import { BrowseTaskList } from "@/components/BrowseTaskList";
import { Button } from "@/components/ui/button";
import { NativeSelect } from "@/components/ui/native-select";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";
import { Spinner } from "@/components/ui/spinner";
import { EmptyState, ErrorState, Modal, Page, PageHeader } from "@/components/ui";

const PAGE_SIZE = 24;

export default function BrowsePage() {
  const games = useBackendQuery<Game[]>(["games"], "list_games");
  const navigate = useUI((s) => s.navigate);
  const sources = useBackendQuery<BrowseSource[]>(["browse-sources"], "browse_list_sources");
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
  const [grant, setGrant] = useState<BrowseGrant | null>(null);
  const [grantBusy, setGrantBusy] = useState(false);
  const [source, setSource] = useState("");
  const [draft, setDraft] = useState("");
  const [text, setText] = useState("");
  const [type, setType] = useState("");
  const [tag, setTag] = useState("");
  const [showFilters, setShowFilters] = useState(false);
  const [revision, setRevision] = useState(0);
  const [error, setError] = useState<unknown>(null);
  const [page, setPage] = useState(0);
  const [taskList, setTaskList] = useState(false);
  const [install, setInstall] = useState<{
    title: string;
    searchId: string;
    itemId: string;
    games: string[];
    gameId: string;
  } | null>(null);
  const search = useBrowseSearch(grant, gameIds, source, text, revision);
  const sourceName = (id: string) => sources.data?.find((s) => s.id === id)?.name ?? id;
  const gameName = (id: string) => games.data?.find((g) => g.id === id)?.name ?? id;
  const all = [
    ...new Map(search.ready.flatMap((frame) => frame.items.map((item) => [item.id, item] as const))).values(),
  ];
  const tags = new Map(all.flatMap((item) => item.tags.map((t) => [t.key, t.label] as const)));
  const filtered = !!type || !!tag;
  const matches = all.filter(
    (item) => (!type || item.typeKey === type) && (!tag || item.tags.some((t) => t.key === tag)),
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
  const canBrowse = !!grant && gameIds.length > 0;
  const reset = () => {
    setPage(0);
    setError(null);
  };
  function newQuery() {
    reset();
    setTag("");
    setType("");
  }
  async function authorize(ids: string[]) {
    if (grantBusy || !ids.length) return;
    setGrantBusy(true);
    setError(null);
    try {
      setGrant(await api<BrowseGrant>("browse_authorize", { sourceIds: ids }));
      setSource("");
    } catch (e) {
      setError(e);
      void sources.refetch();
    } finally {
      setGrantBusy(false);
    }
  }
  async function open(item: BrowseItem) {
    if (!item.canOpenProject || !search.id) return;
    try {
      await api("browse_open_project", { searchId: search.id, itemId: item.id });
    } catch (e) {
      setError(e);
    }
  }
  function next() {
    if (hasLocalNext) setPage(shownPage + 1);
    else if (search.hasNext) {
      search.loadNext();
      if (!fillCurrentPage) setPage(shownPage + 1);
    }
  }
  function reauthorize() {
    newQuery();
    setGrant(null);
    setSource("");
    void sources.refetch();
  }
  const categoryNames = browseTypes();
  return (
    <Page>
      <PageHeader
        title={tr("游览")}
        subtitle={tr("在已连接的 Mod 来源中，为你的游戏寻找新作品。")}
        actions={
          <Button variant="outline" size="sm" onClick={() => setTaskList(true)} disabled={!backendAvailable}>
            <ListTodoIcon />
            {tr("安装任务")}
          </Button>
        }
      />
      {backendAvailable && !!games.data?.length && (
        <section aria-label={tr("游览筛选")} className="flex flex-col gap-2 rounded-xl border bg-card p-3">
          <div className="flex min-w-0 flex-wrap items-center gap-2">
            <form
              className="flex min-w-0 flex-[1_1_18rem] items-center gap-2"
              onSubmit={(e) => {
                e.preventDefault();
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
              <Button type="submit" size="sm" disabled={!canBrowse}>
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
          {canBrowse && (
            <div
              id="browse-filters"
              className={`${showFilters ? "flex" : "hidden"} min-w-0 flex-wrap items-center gap-2 sm:flex`}
            >
              <NativeSelect
                size="sm"
                aria-label={tr("来源")}
                value={source}
                onChange={(e) => {
                  newQuery();
                  setSource(e.target.value);
                }}
              >
                <option value="">{tr("全部来源")}</option>
                {grant.sources.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.name}
                  </option>
                ))}
              </NativeSelect>
              <NativeSelect
                size="sm"
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
              <Button variant="ghost" size="sm" className="sm:ms-auto" onClick={reauthorize}>
                <ShieldCheckIcon />
                {tr("选择来源")}
              </Button>
            </div>
          )}
        </section>
      )}
      <ErrorState error={error || games.error || sources.error || search.error} />
      {!backendAvailable ? (
        <EmptyState title={tr("当前为浏览器预览，请在桌面应用中连接本地服务。")} icon={<CompassIcon />} />
      ) : games.isPending || sources.isPending ? (
        <div role="status" className="flex items-center gap-2 text-sm">
          <Spinner />
          {tr("正在加载...")}
        </div>
      ) : games.isError || sources.isError ? (
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
      ) : !grant ? (
        <BrowseSourceGate
          sources={sources.data ?? []}
          busy={grantBusy}
          allow={(ids) => void authorize(ids)}
          settings={() => navigate("settings")}
          refresh={() => void sources.refetch()}
        />
      ) : (
        <>
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
          {search.error ? (
            <div className="flex flex-wrap gap-2">
              <Button
                variant="outline"
                onClick={() => {
                  reset();
                  setRevision((v) => v + 1);
                }}
              >
                {tr("重试")}
              </Button>
              <Button variant="outline" onClick={reauthorize}>
                {tr("重新授权来源")}
              </Button>
            </div>
          ) : null}
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
                  key={item.id}
                  item={item}
                  source={sourceName(item.sourceId)}
                  gameNames={item.gameIds.map(gameName).join(" · ")}
                  onOpen={() => void open(item)}
                  onInstall={() => {
                    setInstall({
                      title: item.title,
                      itemId: item.id,
                      searchId: search.id,
                      games: item.gameIds,
                      gameId: item.gameIds.length === 1 ? item.gameIds[0] : "",
                    });
                  }}
                />
              ))}
            </div>
          )}
          {!visible.length && !search.busy && !search.error && (
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
              <Button variant="outline" size="sm" disabled={!shownPage} onClick={() => setPage(shownPage - 1)}>
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
          close={() => setTaskList(false)}
          select={(task) => {
            setTaskList(false);
            setInstall({ ...task, games: [task.gameId] });
          }}
        />
      )}
      {install && (
        <Modal
          title={tr("安装 Mod")}
          description={install.title}
          onClose={() => setInstall(null)}
          className="sm:max-w-xl"
        >
          {install.games.length > 1 ? (
            <NativeSelect
              aria-label={tr("目标游戏")}
              value={install.gameId}
              onChange={(e) => setInstall({ ...install, gameId: e.target.value })}
            >
              <option value="" disabled>
                {tr("选择游戏")}
              </option>
              {install.games.map((id) => (
                <option key={id} value={id}>
                  {gameName(id)}
                </option>
              ))}
            </NativeSelect>
          ) : (
            <span className="text-sm text-muted-foreground">{gameName(install.gameId)}</span>
          )}
          {!!install.gameId && (
            <BrowseInstallPanel
              work={{ searchId: install.searchId, itemId: install.itemId }}
              gameId={install.gameId}
              canStart={install.searchId === search.id && !search.error}
            />
          )}
        </Modal>
      )}
    </Page>
  );
}
