import { Fragment, useId } from 'react';
import { CopyIcon, FileIcon, FileTextIcon, Trash2Icon, TriangleAlertIcon } from 'lucide-react';
import { toast } from 'sonner';
import { EmptyState, ErrorState, IconButton, PathText, bytes, date } from '@/components/ui';
import { Alert, AlertDescription } from '@/components/ui/alert';
import { Button } from '@/components/ui/button';
import { Card, CardContent, CardFooter, CardHeader, CardTitle } from '@/components/ui/card';
import { Separator } from '@/components/ui/separator';
import { Spinner } from '@/components/ui/spinner';
import { useBackendQuery } from '@/lib/api';

type GameFileDetailsProps = {
  file: { name: string; path: string; size: number; modified: string } | null;
  busy: boolean;
  writable: boolean;
  onTrash: (path: string) => void;
};

type FileContent = { lines: { line: number; text: string }[]; truncated: boolean };

async function copyPath(path: string) {
  try {
    await navigator.clipboard.writeText(path);
    toast.success('已复制文件路径');
  } catch {
    toast.error('复制文件路径失败，请重试');
  }
}

export function GameFileDetails({ file, busy, writable, onTrash }: GameFileDetailsProps) {
  const previewHeadingId = useId();
  const content = useBackendQuery<FileContent>(['file', file?.path], 'read_file', { path: file?.path }, file !== null);
  // TanStack Query retains the last successful data even after a background refetch fails.
  const preview = content.isError ? undefined : content.data;

  return <Card className="min-w-0 gap-4 py-4">
    <CardHeader className="px-4">
      <CardTitle className="flex items-center gap-2">
        <FileTextIcon aria-hidden="true" className="size-5" />
        文件详情
      </CardTitle>
    </CardHeader>
    <CardContent className="flex min-w-0 flex-col gap-3 px-4">
      {file ? <>
        <Separator />
        <dl className="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-3 gap-y-2 text-sm">
          <dt className="text-muted-foreground">名称</dt>
          <dd className="wrap-anywhere font-medium">{file.name}</dd>
          <dt className="text-muted-foreground">路径</dt>
          <dd className="flex min-w-0 items-center gap-1 rounded-md border bg-muted px-2 py-0.5">
            <PathText className="min-w-0 flex-1" dir="ltr">{file.path}</PathText>
            <IconButton type="button" size="icon-sm" title="复制文件路径" onClick={() => void copyPath(file.path)}>
              <CopyIcon aria-hidden="true" />
            </IconButton>
          </dd>
          <dt className="text-muted-foreground">大小</dt>
          <dd>{bytes(file.size)}</dd>
          <dt className="text-muted-foreground">修改时间</dt>
          <dd className="wrap-anywhere"><time dateTime={file.modified}>{date(file.modified)}</time></dd>
        </dl>
        <Separator />
        <h3 id={previewHeadingId} className="text-sm font-semibold">文件预览</h3>
        {content.isLoading && <p role="status" className="flex items-center gap-2 text-sm text-muted-foreground">
          <Spinner />正在读取文件…
        </p>}
        <ErrorState error={content.error} className="[&_[data-slot=alert-description]]:wrap-anywhere" />
        {preview && <>
          <div key={file.path} role="region" aria-labelledby={previewHeadingId} tabIndex={0}
            className="file-viewer max-h-[420px] min-w-0 overflow-auto rounded-md border bg-muted focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring">
            {preview.lines.length === 0 ? <p className="p-3 text-sm text-muted-foreground">文件为空</p> :
              <code className="grid grid-cols-[minmax(2.5rem,max-content)_minmax(0,1fr)] py-2 font-mono text-xs leading-5">
                {preview.lines.map(line => <Fragment key={line.line}>
                  <span aria-hidden="true" className="border-e px-2 text-right text-muted-foreground tabular-nums select-none">{line.line}</span>
                  <span className="wrap-anywhere px-3 whitespace-pre-wrap">{line.text}</span>
                </Fragment>)}
              </code>}
          </div>
          {preview.truncated && <Alert role="note" variant="warning"><TriangleAlertIcon /><AlertDescription>内容已截断</AlertDescription></Alert>}
        </>}
      </> : <EmptyState title="未选择文件" icon={<FileIcon />} />}
    </CardContent>
    {file && <CardFooter className="flex-col items-start gap-3 px-4">
      <Separator />
      <Button type="button" variant="destructive" disabled={!writable || busy} onClick={() => onTrash(file.path)}>
        <Trash2Icon data-icon="inline-start" aria-hidden="true" />
        移入回收站
      </Button>
    </CardFooter>}
  </Card>;
}
