import { Badge } from '@/components/ui/badge';

export type PublicationListing = {
  title: string;
  summary: string;
  description: string;
  tags: string[];
  coverPng: string;
};

export function PublicationListingPreview({listing}: {listing: PublicationListing}) {
  const {title, summary, description, tags, coverPng} = listing;
  const isPng = typeof coverPng === 'string' && coverPng.startsWith('iVBORw0KGgo') && /^[A-Za-z0-9+/]+={0,2}$/.test(coverPng);
  return <section aria-label="插件发布内容" className="flex min-w-0 flex-col gap-4">
    <div className="grid min-w-0 items-center gap-4 sm:grid-cols-[11.5rem_minmax(0,1fr)]">
      <div className="flex aspect-video items-center justify-center overflow-hidden rounded-md bg-muted">
        {isPng ? <img src={`data:image/png;base64,${coverPng}`} alt={`${title}的封面`} className="size-full object-cover"/>
          : <span className="text-sm text-muted-foreground">封面无法显示</span>}
      </div>
      <div className="flex min-w-0 flex-col items-start gap-1.5">
        <h3 className="text-xl font-semibold tracking-tight wrap-anywhere">{title}</h3>
        <p className="text-sm whitespace-pre-wrap text-muted-foreground wrap-anywhere">{summary}</p>
        {tags.length > 0 && <ul aria-label="标签" className="flex max-w-full flex-wrap gap-1.5">
          {tags.map(tag => <li key={tag} className="max-w-full"><Badge variant="outline" className="max-w-full rounded-sm whitespace-normal wrap-anywhere">{tag}</Badge></li>)}
        </ul>}
      </div>
    </div>
    <div className="flex min-w-0 flex-col gap-1.5">
      <h4 className="text-sm font-medium text-muted-foreground">描述</h4>
      <p className="text-sm leading-relaxed whitespace-pre-wrap wrap-anywhere">{description}</p>
    </div>
  </section>;
}
