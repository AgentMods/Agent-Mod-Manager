import { tr } from "@/lib/i18n";
import { useEffect, useState } from 'react';
import { useQueries, useQueryClient, type UseQueryResult } from '@tanstack/react-query';
import { ChevronRightIcon, FileIcon, FolderIcon, FolderOpenIcon, FolderTreeIcon, RefreshCwIcon, SearchIcon } from 'lucide-react';
import { GameFileDetails } from '@/components/GameFileDetails';
import { bytes, date, ErrorState, IconButton, PathText } from '@/components/ui';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardHeader, CardTitle } from '@/components/ui/card';
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible';
import { InputGroup, InputGroupAddon, InputGroupInput } from '@/components/ui/input-group';
import { NativeSelect, NativeSelectOption } from '@/components/ui/native-select';
import { Spinner } from '@/components/ui/spinner';
import { api, backendAvailable } from '@/lib/api';
import { cn } from '@/lib/utils';
import { displayPath } from '@/lib/paths';

type FileRoot = { name: string; path: string };
type DirEntry = FileRoot & { is_dir: boolean; size: number; modified: string };
type FileNode = { type: 'file'; entry: DirEntry };
type FolderNode = FileRoot & {
  type: 'folder'; open: boolean; children: (FolderNode | FileNode)[];
  loading: boolean; error?: Error | null; modified?: string;
};

// Read only the reachable branches. Filtering keeps the ancestors of matching files.
function buildTree(root: FileRoot, directories: Map<string, UseQueryResult<DirEntry[]>>, search: string) {
  const reachable = new Set<string>();
  const files = new Map<string, DirEntry>();
  function visit(folder: FileRoot, modified?: string): FolderNode {
    const listing = directories.get(folder.path);
    const children: FolderNode['children'] = [];
    if (listing) {
      reachable.add(folder.path);
      for (const entry of listing.isError ? [] : listing.data ?? []) {
        const matches = entry.name.toLowerCase().includes(search);
        if (entry.is_dir) {
          if (reachable.has(entry.path)) continue;
          const child = visit(entry, entry.modified);
          if (matches || child.children.length || child.error || child.loading) children.push(child);
        } else {
          files.set(entry.path, entry);
          if (matches) children.push({ type: 'file', entry });
        }
      }
    }
    return { type: 'folder', name: folder.name, path: folder.path, modified, open: !!listing, loading: !!listing?.isLoading, error: listing?.error, children };
  }
  return { root: visit(root), reachable, files };
}

const rowClass = 'w-full min-w-0 justify-start gap-2 font-normal focus-visible:ring-inset';
const sizeClass = 'hidden w-16 shrink-0 text-right text-xs tabular-nums text-muted-foreground @xs:block';
const dateClass = 'hidden w-32 shrink-0 text-right text-xs tabular-nums text-muted-foreground @xl:block';

function FileTreeNode({ node, selected, onSelect, onToggle, searching }: {
  node: FileNode | FolderNode; selected: string | null; onSelect: (path: string) => void;
  onToggle: (path: string, open: boolean) => void; searching: boolean;
}) {
  if (node.type === 'file') {
    const { entry } = node;
    return <Button type="button" variant="ghost" size="sm" aria-label={entry.name} title={entry.name}
      aria-current={selected === entry.path ? true : undefined} onClick={() => onSelect(entry.path)}
      className={cn(rowClass, 'aria-[current=true]:bg-accent aria-[current=true]:text-accent-foreground')}>
      <FileIcon aria-hidden="true" className="ml-5 text-muted-foreground" />
      <span className="min-w-0 flex-1 truncate text-left">{entry.name}</span>
      <span className={sizeClass}>{bytes(entry.size)}</span><span className={dateClass}>{date(entry.modified)}</span>
    </Button>;
  }
  const Folder = node.open ? FolderOpenIcon : FolderIcon;
  return <Collapsible open={node.open} onOpenChange={open => onToggle(node.path, open)}>
    <CollapsibleTrigger asChild>
      <Button type="button" variant="ghost" size="sm" className={rowClass} aria-label={node.name} title={displayPath(node.path)}>
        <ChevronRightIcon aria-hidden="true" className={cn('size-3 text-muted-foreground transition-transform motion-reduce:transition-none', node.open && 'rotate-90')} />
        <Folder aria-hidden="true" /><span className="min-w-0 flex-1 truncate text-left font-medium">{node.name}</span>
        <span className={sizeClass}>—</span><span className={dateClass}>{node.modified && date(node.modified)}</span>
      </Button>
    </CollapsibleTrigger>
    <CollapsibleContent className="motion-safe:overflow-hidden motion-safe:data-[state=closed]:animate-collapsible-up motion-safe:data-[state=open]:animate-collapsible-down">
      <div className="ml-3 flex min-w-0 flex-col border-l pl-1 sm:ml-4">
        {node.loading && <p role="status" className="flex items-center gap-2 px-3 py-2 text-xs text-muted-foreground"><Spinner />{tr("正在读取目录…")}</p>}
        <ErrorState error={node.error} />
        {!node.loading && !node.error && !node.children.length && <p role="status" className="px-3 py-2 text-xs text-muted-foreground">
          {searching ? tr("没有匹配的文件或文件夹") : tr("目录中没有可显示的文件")}
        </p>}
        {node.children.map(child => <FileTreeNode key={child.type === 'file' ? child.entry.path : child.path}
          node={child} selected={selected} onSelect={onSelect} onToggle={onToggle} searching={searching} />)}
      </div>
    </CollapsibleContent>
  </Collapsible>;
}

export function GameFileBrowser({ root, roots, onRootChange, active, busy, writable, onTrash }: {
  root: FileRoot; roots: FileRoot[]; onRootChange: (path: string) => void; active: boolean;
  busy: boolean; writable: boolean; onTrash: (path: string) => void;
}) {
  const client = useQueryClient();
  const [expanded, setExpanded] = useState<string[]>([root.path]);
  const [search, setSearch] = useState('');
  const [selected, setSelected] = useState<string | null>(null);
  const queries = useQueries({ queries: expanded.map(path => ({
    queryKey: ['directory', root.path, path], enabled: active && backendAvailable, retry: false,
    queryFn: async () => {
      const entries = await api<DirEntry[]>('read_dir', { path });
      return entries.sort((a, b) => Number(b.is_dir) - Number(a.is_dir));
    },
  })) });
  const term = search.trim().toLowerCase();
  const tree = buildTree(root, new Map(expanded.map((path, index) => [path, queries[index]])), term);
  const file = selected ? tree.files.get(selected) ?? null : null;
  useEffect(() => {
    if (tree.reachable.size !== expanded.length) setExpanded(expanded.filter(path => tree.reachable.has(path)));
  }, [expanded, tree.reachable]);
  useEffect(() => {
    if (selected && !file) {
      setSelected(null);
      const filters = { queryKey: ['file', selected], exact: true };
      void client.cancelQueries(filters);
      client.removeQueries(filters);
    }
  }, [client, file, selected]);
  if (!active) return null;
  const fetching = queries.some(query => query.isFetching);
  function refresh() {
    void client.invalidateQueries({ queryKey: ['directory', root.path] });
    if (file) void client.invalidateQueries({ queryKey: ['file', file.path], exact: true });
  }
  return <div className="grid min-w-0 items-start gap-4 lg:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
    <Card className="min-w-0 gap-4 py-4">
      <CardHeader className="gap-3 px-4">
        <div className="flex min-w-0 items-start justify-between gap-2">
          <div className="flex min-w-0 flex-col gap-2">
            <CardTitle className="flex items-center gap-2"><FolderTreeIcon aria-hidden="true" className="size-4" />{tr("文件浏览器")}</CardTitle>
            <PathText>{displayPath(root.path)}</PathText>
          </div>
          <IconButton title={tr("刷新目录")} disabled={!backendAvailable || fetching || expanded.length === 0} onClick={refresh}>
            <RefreshCwIcon className={fetching ? 'motion-safe:animate-spin' : undefined} />
          </IconButton>
        </div>
        <div className="flex min-w-0 flex-wrap gap-2">
          {roots.length > 1 && <NativeSelect aria-label={tr("浏览根目录")} value={root.path} onChange={event => onRootChange(event.target.value)}>
            {roots.map(option => <NativeSelectOption key={option.path + option.name} value={option.path}>{option.name}</NativeSelectOption>)}
          </NativeSelect>}
          <InputGroup className="min-w-32 flex-1"><InputGroupAddon><SearchIcon /></InputGroupAddon>
            <InputGroupInput type="search" aria-label={tr("搜索已展开目录")} placeholder={tr("搜索已展开目录…")} value={search} onChange={event => setSearch(event.target.value)} />
          </InputGroup>
        </div>
      </CardHeader>
      <CardContent className="px-4">
        <div className="@container min-w-0 overflow-hidden rounded-md border">
          <div aria-hidden="true" className="flex items-center gap-2 border-b bg-muted/50 px-3 py-2 text-xs text-muted-foreground">
            <span className="flex-1">{tr("名称")}</span><span className={sizeClass}>{tr("大小")}</span><span className={dateClass}>{tr("修改时间")}</span>
          </div>
          <div role="group" aria-label={tr("文件树")} aria-busy={fetching} className="max-h-[60dvh] overflow-auto p-1">
            <FileTreeNode node={tree.root} selected={selected} onSelect={setSelected} searching={!!term}
              onToggle={(path, open) => setExpanded(current => open ? [...current, path] : current.filter(item => item !== path))} />
          </div>
        </div>
      </CardContent>
    </Card>
    <GameFileDetails file={file} busy={busy} writable={writable} onTrash={onTrash} />
  </div>;
}
