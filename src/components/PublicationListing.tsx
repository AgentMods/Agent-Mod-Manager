import { tr } from "@/lib/i18n";
import { GameCover } from './GameCover';
import { Badge } from '@/components/ui/badge';

export type PublicationListing = {
  title: string;
  summary: string;
  description: string;
  tags: string[];
  coverUrl?: string | null;
  coverPng?: string; // Read-only historical format; never render inline artwork.
};

// Historical receipts kept the game's original URL in the immutable manifest, not the listing.
function manifestCover(files: unknown): string | null {
  if (!Array.isArray(files)) return null;
  const file = files.find(item => item?.name === 'adapter.json');
  if (typeof file?.text !== 'string') return null;
  try { const manifest: unknown = JSON.parse(file.text); return manifest && typeof manifest === 'object' && 'coverUrl' in manifest && typeof manifest.coverUrl === 'string' ? manifest.coverUrl : null; }
  catch { return null; }
}

export function PublicationListingPreview({listing, files}: {listing: PublicationListing; files?: unknown}) {
  const {title, summary, description, tags} = listing;
  const cover = listing.coverPng ? manifestCover(files) : listing.coverUrl;
  return <section aria-label={tr("插件发布内容")} className="flex min-w-0 flex-col gap-4">
    <div className="grid min-w-0 items-center gap-4 sm:grid-cols-[11.5rem_minmax(0,1fr)]">
      <GameCover name={title} url={cover} className="aspect-video rounded-md" />
      <div className="flex min-w-0 flex-col items-start gap-1.5">
        <h3 className="text-xl font-semibold tracking-tight wrap-anywhere">{title}</h3>
        <p className="text-sm whitespace-pre-wrap text-muted-foreground wrap-anywhere">{summary}</p>
        {tags.length > 0 && <ul aria-label={tr("标签")} className="flex max-w-full flex-wrap gap-1.5">
          {tags.map(tag => <li key={tag} className="max-w-full"><Badge variant="outline" className="max-w-full rounded-sm whitespace-normal wrap-anywhere">{tag}</Badge></li>)}
        </ul>}
      </div>
    </div>
    <div className="flex min-w-0 flex-col gap-1.5">
      <h4 className="text-sm font-medium text-muted-foreground">{tr("描述")}</h4>
      <p className="text-sm leading-relaxed whitespace-pre-wrap wrap-anywhere">{description}</p>
    </div>
  </section>;
}
