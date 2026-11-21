import { useEffect, useId, useRef, useState } from "react";
import { CheckIcon, ChevronLeftIcon, ChevronRightIcon, CircleHelpIcon, ClipboardListIcon, RotateCcwIcon, SquareIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldLabel } from "@/components/ui/field";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Separator } from "@/components/ui/separator";
import { Spinner } from "@/components/ui/spinner";
import { Textarea } from "@/components/ui/textarea";
import { TraceCollapsible, TraceCollapsibleContent, TraceCollapsibleTrigger, TraceDescription, TraceIcon, TraceLine, TraceTitle } from "@/components/ui/trace";
import type { AgentEvent } from "@/lib/types";
import { ErrorState } from "./ui";

const CUSTOM = "custom";
const MAX_ANSWER_LENGTH = 2000;

type Question = { question: string; options: string[] };
type Choice = { value: string; text?: string };
export type Answer = string | null;
type QuestionOutcome =
  | { kind: "answered"; answers?: (Answer | undefined)[]; syncing?: boolean }
  | { kind: "skipped" }
  | { kind: "closed"; label: string };

function readQuestion(value: unknown): Question | null {
  if (!value || typeof value !== "object") return null;
  const { question, options } = value as Record<string, unknown>;
  if (typeof question !== "string" || !question.trim()) return null;
  return { question, options: Array.isArray(options) ? options.filter((option): option is string => typeof option === "string" && !!option.trim()) : [] };
}

function questionOutcome(request: AgentEvent, events: AgentEvent[], submitted?: Answer[]): QuestionOutcome | null {
  const resolution = events.find((event) => event.kind === "approval-resolved" && event.payload.approvalId === request.payload.approvalId);
  const status = resolution?.payload.status;
  if (status === "cancelled") return { kind: "closed", label: "已取消或过期" };
  if (status === "rejected") return { kind: "skipped" };

  const result = events.find((event) => event.kind === "tool-result" && event.turnId === request.turnId &&
    event.seq > request.seq && event.payload.toolCallId === request.payload.toolCallId);
  const answer = (result?.payload.result as { answer?: unknown } | undefined)?.answer;
  if (Array.isArray(answer)) {
    return { kind: "answered", answers: answer.map((item) => item === null || typeof item === "string" ? item : undefined) };
  }
  if (typeof answer === "string") return { kind: "answered", answers: [answer] };
  if (answer === null) return { kind: "skipped" };

  const done = events.find((event) => event.kind === "done" && event.turnId === request.turnId && event.seq > request.seq);
  if (status === "approved" || submitted) return { kind: "answered", answers: submitted, syncing: !result && !done };
  if (done) return { kind: "closed", label: done.payload.status === "cancelled" ? "已取消" : "已结束" };
  return null;
}

export function describeQuestionnaire(request: AgentEvent, events: AgentEvent[], archived: boolean, submitted?: Answer[]) {
  const preview = request.payload.preview as Record<string, unknown> | undefined;
  const structured = Array.isArray(preview?.questions);
  const questions = structured ? (preview?.questions as unknown[]).map(readQuestion) : [readQuestion(preview)];
  const title = typeof preview?.title === "string" && preview.title.trim() ? preview.title : "Agent 等待你的回答";
  const outcome = questionOutcome(request, events, submitted) ?? (archived ? { kind: "closed" as const, label: "对话已归档" } : null);
  const valid = questions.length > 0 && questions.every(Boolean);
  const skippedAll = outcome?.kind === "answered" && outcome.answers?.length === questions.length && outcome.answers.every((answer) => answer === null);
  const canReanswer = valid && !archived && (outcome?.kind === "closed" || outcome?.kind === "skipped" || skippedAll);
  return { request, title, questions, structured, valid, outcome, canReanswer };
}

type Questionnaire = ReturnType<typeof describeQuestionnaire>;

export function QuestionnaireTrace({ questionnaire, onReanswer, disabled }: {
  questionnaire: Questionnaire;
  onReanswer?: () => void;
  disabled: boolean;
}) {
  const { questions, outcome, canReanswer } = questionnaire;
  if (!outcome) return null;
  const count = questions.filter(Boolean).length;
  if (!count) return null;
  const answers = outcome.kind === "answered" ? outcome.answers : undefined;
  return <TraceCollapsible>
    <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
      <TraceCollapsibleTrigger className="w-fit gap-2 px-1.5">
        <TraceLine>
          <TraceIcon><CircleHelpIcon /></TraceIcon>
          <TraceTitle>已询问 {count} 个问题</TraceTitle>
          {outcome.kind === "closed" && <TraceDescription>{outcome.label}</TraceDescription>}
        </TraceLine>
      </TraceCollapsibleTrigger>
      {canReanswer && onReanswer && <Button type="button" variant="outline" size="lg" disabled={disabled}
        title="在当前对话中补充回答" onClick={onReanswer}>
        <RotateCcwIcon data-icon="inline-start" />重新回答
      </Button>}
    </div>
    <TraceCollapsibleContent>
      <dl className="flex flex-col gap-3 text-sm wrap-anywhere whitespace-pre-wrap">
        {questions.map((item, index) => {
          if (!item) return null;
          const answer = answers?.[index];
          let reply = answer;
          if (outcome.kind === "closed") reply = "未回答";
          else if (outcome.kind === "skipped" || answer === null) reply = "已跳过";
          else if (answer === undefined) reply = outcome.kind === "answered" && outcome.syncing ? "正在同步回答…" : "无可用回答记录";
          return <div key={index} className="flex min-w-0 flex-col gap-1">
            <dt className="font-medium text-foreground/85">{item.question}</dt>
            <dd>{reply}</dd>
          </div>;
        })}
      </dl>
    </TraceCollapsibleContent>
  </TraceCollapsible>;
}

export function AgentQuestionnaire({ questionnaire, disabled, onSubmit, onCancel }: {
  questionnaire: Questionnaire;
  disabled: boolean;
  onSubmit: (answers: Answer[]) => Promise<void>;
  onCancel: () => void;
}) {
  const { questions, title } = questionnaire;
  const reanswering = !!questionnaire.outcome;
  const id = useId();
  const heading = useRef<HTMLHeadingElement>(null);
  const body = useRef<HTMLDivElement>(null);
  const sending = useRef(false);
  const [step, setStep] = useState(0);
  const [draft, setDraft] = useState<Record<number, Choice | null>>({});
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const busy = submitting || disabled;
  const question = questions[step];
  const finalStep = step + 1 === questions.length;
  const selection = draft[step];
  const customText = selection?.text ?? "";
  const currentAnswer = answerAt(step);

  useEffect(() => {
    body.current?.scrollTo({ top: 0 });
    heading.current?.focus({ preventScroll: true });
  }, [step]);

  function answerAt(index: number) {
    const choice = draft[index];
    if (!choice) return null;
    if (choice.value === CUSTOM) return choice.text?.trim() || null;
    return questions[index]?.options[Number(choice.value)] ?? null;
  }

  async function next(skip: boolean) {
    if (sending.current || busy || !question) return;
    const answer = skip ? null : currentAnswer;
    if (!skip && !answer) return;
    if (!finalStep) {
      if (skip) setDraft({ ...draft, [step]: null });
      setStep(step + 1);
      return;
    }
    const answers = questions.map((_, index) => index === step ? answer : answerAt(index));
    sending.current = true;
    setSubmitting(true);
    setError(null);
    try { await onSubmit(answers); }
    catch (failure) { setError(failure); }
    finally { sending.current = false; setSubmitting(false); }
  }

  if (!question) return null;
  return <Card role="region" aria-labelledby={`${id}-title`} className="min-h-0 w-full min-w-0 gap-0 overflow-hidden rounded-2xl py-0">
    <div ref={body} className="min-h-0 overflow-y-auto overscroll-contain">
      <CardHeader className="px-4 py-3.5 sm:px-5">
        <CardTitle id={`${id}-title`} className="flex items-start gap-2 text-base leading-snug">
          <ClipboardListIcon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
          <span className="min-w-0 wrap-anywhere">{title}</span>
        </CardTitle>
        {reanswering && <CardDescription>回答将作为新消息发送到当前对话。</CardDescription>}
      </CardHeader>
      <Separator />
      <CardContent className="flex flex-col gap-3 px-4 py-4 sm:px-5">
        <div className="flex min-w-0 items-start justify-between gap-3">
          <h3 id={`${id}-question`} ref={heading} tabIndex={-1} className="min-w-0 font-medium leading-relaxed wrap-anywhere whitespace-pre-line outline-none">
            {question.question}
          </h3>
          <span className="shrink-0 pt-0.5 text-xs text-muted-foreground tabular-nums" aria-label={`第 ${step + 1} 题，共 ${questions.length} 题`}>{step + 1} / {questions.length}</span>
        </div>
        {question.options.length > 0 && <RadioGroup key={step} aria-labelledby={`${id}-question`} className="gap-1.5"
          value={selection?.value ?? ""} disabled={busy}
          onValueChange={(value) => setDraft({ ...draft, [step]: { value, text: customText } })}>
          {[...question.options.map((label, index) => ({value: String(index), label})), {value: CUSTOM, label: "自己填写"}].map(({value, label}) => {
            const itemId = `${id}-${step}-${value}`;
            return <Label key={value} htmlFor={itemId}
              className="flex min-h-11 min-w-0 cursor-pointer items-center gap-2.5 rounded-xl border p-2.5 leading-snug hover:bg-muted/60 has-[[data-state=checked]]:border-ring has-[[data-state=checked]]:bg-accent/50">
              <RadioGroupItem id={itemId} value={value} aria-labelledby={`${itemId}-label`} />
              <span id={`${itemId}-label`} className="min-w-0 wrap-anywhere">{label}</span>
            </Label>;
          })}
        </RadioGroup>}
        {(!question.options.length || selection?.value === CUSTOM) && <Field>
          <FieldLabel htmlFor={`${id}-answer`}>填写回答</FieldLabel>
          <Textarea id={`${id}-answer`} rows={3} maxLength={MAX_ANSWER_LENGTH} value={customText} disabled={busy}
            placeholder="输入你的回答…" onChange={(event) => setDraft({ ...draft, [step]: { value: CUSTOM, text: event.target.value } })}/>
        </Field>}
        <ErrorState error={error} />
      </CardContent>
    </div>
    <Separator />
    <CardFooter className="shrink-0 flex-wrap justify-between gap-2 px-4 py-2.5 sm:px-5">
      <div className="flex flex-wrap items-center gap-1">
        <Button type="button" size="sm" variant="ghost" disabled={submitting} onClick={onCancel}>
          {!reanswering && <SquareIcon data-icon="inline-start" />}
          {reanswering ? "取消重新回答" : "停止生成"}
        </Button>
        {step > 0 && <Button type="button" size="sm" variant="ghost" disabled={busy} onClick={() => setStep(step - 1)}>
          <ChevronLeftIcon data-icon="inline-start" />上一步
        </Button>}
        <Button type="button" size="sm" variant="ghost" disabled={busy} onClick={() => { void next(true); }}>跳过</Button>
      </div>
      <Button type="button" size="sm" disabled={busy || !currentAnswer} onClick={() => { void next(false); }}>
        {busy ? <Spinner data-icon="inline-start" /> : finalStep && <CheckIcon data-icon="inline-start" />}
        {finalStep ? "提交回答" : "下一步"}
        {!finalStep && <ChevronRightIcon data-icon="inline-end" />}
      </Button>
    </CardFooter>
  </Card>;
}
