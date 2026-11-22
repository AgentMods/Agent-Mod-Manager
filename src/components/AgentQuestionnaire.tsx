import { useEffect, useId, useRef, useState } from "react";
import { CheckIcon, ChevronLeftIcon, ChevronRightIcon, CircleHelpIcon, ClipboardListIcon, RotateCcwIcon, SquareIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { Field, FieldLabel, FieldSet } from "@/components/ui/field";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Separator } from "@/components/ui/separator";
import { Spinner } from "@/components/ui/spinner";
import { Textarea } from "@/components/ui/textarea";
import { TraceCollapsible, TraceCollapsibleContent, TraceCollapsibleTrigger, TraceDescription, TraceIcon, TraceLine, TraceTitle } from "@/components/ui/trace";
import type { AgentEvent } from "@/lib/types";
import { ErrorState } from "./ui";

const CUSTOM = "custom";
const OPTIONS_PER_PAGE = 50;

type Question = { question: string; options: string[]; multiSelect: boolean };
type OptionKey = number | typeof CUSTOM;
type Choice = { values: OptionKey[]; text: string };
export type Answer = string | string[] | null;
type QuestionOutcome =
  | { kind: "answered"; answers?: (Answer | undefined)[]; syncing?: boolean }
  | { kind: "skipped" }
  | { kind: "closed"; label: string };

// Older previews sometimes stored the selection mode only as a standalone hint in the question.
function hasMultiSelectHint(question: string) {
  for (const [word] of question.matchAll(/[^\s()（）【】[\]，,。.!！?？:：;；、]+/gu)) {
    if (word === "多选" || word === "可多选" || word === "可以多选") return true;
  }
  return false;
}

function readQuestion(value: unknown): Question | null {
  if (!value || typeof value !== "object") return null;
  const { question, options, multiSelect } = value as Record<string, unknown>;
  if (typeof question !== "string" || !question.trim()) return null;
  const labels = Array.isArray(options) ? options.filter((option): option is string => typeof option === "string" && !!option.trim()) : [];
  const multiple = labels.length > 0 && (multiSelect === true || (multiSelect === undefined && hasMultiSelectHint(question)));
  return { question, options: labels, multiSelect: multiple };
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
    return { kind: "answered", answers: answer.map((item: unknown) => {
      if (item === null || typeof item === "string") return item;
      if (Array.isArray(item) && item.length && item.every((label) => typeof label === "string")) return item;
      return undefined;
    }) };
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
      <QuestionnaireAnswers questions={questions} outcome={outcome} />
    </TraceCollapsibleContent>
  </TraceCollapsible>;
}

// Mount the detailed list only when the collapsible is open.
function QuestionnaireAnswers({ questions, outcome }: {questions: Questionnaire["questions"]; outcome: QuestionOutcome}) {
  const answers = outcome.kind === "answered" ? outcome.answers : undefined;
  return <dl className="flex flex-col gap-3 text-sm wrap-anywhere whitespace-pre-wrap">
    {questions.map((item, index) => {
      if (!item) return null;
      const answer = answers?.[index];
      let reply = answer;
      if (outcome.kind === "closed") reply = "未回答";
      else if (outcome.kind === "skipped" || answer === null) reply = "已跳过";
      else if (answer === undefined) reply = outcome.kind === "answered" && outcome.syncing ? "正在同步回答…" : "无可用回答记录";
      return <div key={index} className="flex min-w-0 flex-col gap-1">
        <dt className="font-medium text-foreground/85">{item.question}</dt>
        <dd>{Array.isArray(reply) ? <ul className="list-disc space-y-1 ps-5">
          {reply.map((label, position) => <li key={position}>{label}</li>)}
        </ul> : reply}</dd>
      </div>;
    })}
  </dl>;
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
  const [optionPage, setOptionPage] = useState(0);
  const [draft, setDraft] = useState<Record<number, Choice | null>>({});
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const busy = submitting || disabled;
  const question = questions[step];
  const finalStep = step + 1 === questions.length;
  const selection = draft[step];
  const optionStart = optionPage * OPTIONS_PER_PAGE;
  const optionPages = Math.ceil((question?.options.length ?? 0) / OPTIONS_PER_PAGE);
  const visibleOptions = question?.options.slice(optionStart, optionStart + OPTIONS_PER_PAGE) ?? [];
  const chosenValues = selection?.values ?? [];
  const customText = selection?.text ?? "";
  const currentAnswer = answerAt(step);

  useEffect(() => {
    body.current?.scrollTo({ top: 0 });
    heading.current?.focus({ preventScroll: true });
  }, [step]);

  function answerAt(index: number): Answer {
    const choice = draft[index];
    const item = questions[index];
    if (!item || !choice?.values.length) return null;
    const answers = choice.values.filter((value): value is number => value !== CUSTOM)
      .sort((a, b) => a - b).map((value) => item.options[value]);
    if (choice.values.includes(CUSTOM)) {
      const text = choice.text.trim();
      if (!text) return null;
      if (!answers.includes(text)) answers.push(text);
    }
    return item.multiSelect ? answers : answers[0];
  }

  function updateChoice(values: OptionKey[], text = customText) {
    setDraft({ ...draft, [step]: { values, text } });
  }

  function goToStep(index: number) {
    const firstOption = draft[index]?.values.find((value): value is number => value !== CUSTOM);
    setOptionPage(Math.floor((firstOption ?? 0) / OPTIONS_PER_PAGE));
    setStep(index);
  }

  async function next(skip: boolean) {
    if (sending.current || busy || !question) return;
    const answer = skip ? null : currentAnswer;
    if (!skip && !answer) return;
    if (!finalStep) {
      if (skip && selection) setDraft({ ...draft, [step]: null });
      goToStep(step + 1);
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
  const options = [...visibleOptions.map((label, index) => ({ value: optionStart + index, label })), { value: CUSTOM, label: "自己填写" } as const];
  const optionRows = options.map(({ value, label }) => {
    const itemId = `${id}-${step}-${value}`;
    return <Label key={value} htmlFor={itemId}
      className="flex min-h-11 min-w-0 cursor-pointer items-center gap-2.5 rounded-xl border p-2.5 leading-snug hover:bg-muted/60 has-[[data-state=checked]]:border-ring has-[[data-state=checked]]:bg-accent/50">
      {question.multiSelect
        ? <Checkbox id={itemId} aria-labelledby={`${itemId}-label`} disabled={busy} checked={chosenValues.includes(value)}
            onCheckedChange={(checked) => updateChoice(checked === true ? [...chosenValues, value] : chosenValues.filter((item) => item !== value))} />
        : <RadioGroupItem id={itemId} value={String(value)} aria-labelledby={`${itemId}-label`} />}
      <span id={`${itemId}-label`} className="min-w-0 wrap-anywhere">{label}</span>
    </Label>;
  });
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
        {optionPages > 1 && <div className="flex flex-wrap items-center gap-2">
          <Button type="button" size="sm" variant="outline" disabled={busy || optionPage === 0} onClick={() => setOptionPage(optionPage - 1)}>上一页选项</Button>
          <span role="status" className="text-xs text-muted-foreground tabular-nums">第 {optionPage + 1} / {optionPages} 页，共 {question.options.length} 个选项</span>
          <Button type="button" size="sm" variant="outline" disabled={busy || optionPage + 1 === optionPages} onClick={() => setOptionPage(optionPage + 1)}>下一页选项</Button>
        </div>}
        {question.options.length > 0 && (question.multiSelect
          ? <FieldSet key={step} aria-labelledby={`${id}-question`} className="min-w-0 gap-1.5">
              <span className="text-xs text-muted-foreground" role="status">可多选 · 已选 {chosenValues.length} 项</span>
              {optionRows}
            </FieldSet>
          : <RadioGroup key={step} aria-labelledby={`${id}-question`} className="gap-1.5"
              value={String(chosenValues[0] ?? "")} disabled={busy}
              onValueChange={(value) => updateChoice([value === CUSTOM ? CUSTOM : Number(value)])}>
              {optionRows}
            </RadioGroup>)}
        {(!question.options.length || chosenValues.includes(CUSTOM)) && <Field>
          <FieldLabel htmlFor={`${id}-answer`}>填写回答</FieldLabel>
          <Textarea id={`${id}-answer`} rows={3} value={customText} disabled={busy}
            placeholder="输入你的回答…" onChange={(event) => updateChoice(question.options.length ? chosenValues : [CUSTOM], event.target.value)}/>
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
        {step > 0 && <Button type="button" size="sm" variant="ghost" disabled={busy} onClick={() => goToStep(step - 1)}>
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
