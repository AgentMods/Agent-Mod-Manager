import { useId, useRef, useState } from "react";
import { CheckIcon, ChevronLeftIcon, ChevronRightIcon, ClipboardListIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardFooter, CardHeader, CardTitle } from "@/components/ui/card";
import { Field, FieldLabel } from "@/components/ui/field";
import { Label } from "@/components/ui/label";
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group";
import { Separator } from "@/components/ui/separator";
import { Spinner } from "@/components/ui/spinner";
import { Textarea } from "@/components/ui/textarea";
import { replayConversation } from "@/lib/agent-store";
import { api } from "@/lib/api";
import type { AgentEvent } from "@/lib/types";
import { ErrorState } from "./ui";

const CUSTOM = "custom";
const MAX_ANSWER_LENGTH = 2000;

type Question = { question: string; options: string[] };
type Choice = { value: string; text?: string };

function readQuestion(value: unknown): Question | null {
  if (!value || typeof value !== "object") return null;
  const { question, options } = value as Record<string, unknown>;
  if (typeof question !== "string" || !question.trim()) return null;
  return { question, options: Array.isArray(options) ? options.filter((option): option is string => typeof option === "string" && !!option.trim()) : [] };
}

export function questionStatus(request: AgentEvent, events: AgentEvent[]) {
  const resolution = events.find((event) => event.kind === "approval-resolved" && event.payload.approvalId === request.payload.approvalId);
  if (resolution) {
    return resolution.payload.status === "approved" ? "已提交回答" : resolution.payload.status === "rejected" ? "已跳过" : "已取消或过期";
  }
  return events.some((event) => event.turnId === request.turnId && event.seq > request.seq && event.kind === "done") ? "已结束" : null;
}

export function AgentQuestionnaire({ request, status }: { request: AgentEvent; status: string | null }) {
  const id = useId();
  const heading = useRef<HTMLHeadingElement>(null);
  const sending = useRef(false);
  const [step, setStep] = useState(0);
  const [draft, setDraft] = useState<Record<number, Choice | null>>({});
  const [busy, setBusy] = useState(false);
  const [submitted, setSubmitted] = useState<string>();
  const [error, setError] = useState<unknown>(null);
  const preview = request.payload.preview as Record<string, unknown> | undefined;
  const structured = Array.isArray(preview?.questions);
  const questions = structured ? (preview?.questions as unknown[]).map(readQuestion) : [readQuestion(preview)];
  const question = questions[step];
  const title = typeof preview?.title === "string" && preview.title.trim() ? preview.title : "Agent 等待你的回答";
  const finalStep = step + 1 === questions.length;
  const selection = draft[step];
  const customText = selection?.text ?? "";
  const currentAnswer = answerAt(step);
  const complete = status || submitted;

  function answerAt(index: number) {
    const choice = draft[index];
    if (!choice) return null;
    if (choice.value === CUSTOM) return choice.text?.trim() || null;
    return questions[index]?.options[Number(choice.value)] ?? null;
  }

  function moveBy(offset: number) {
    setStep(step + offset);
    heading.current?.focus({ preventScroll: true });
  }

  async function next(skip: boolean) {
    if (sending.current || complete) return;
    const answer = skip ? null : currentAnswer;
    if (!skip && !answer) return;
    if (!finalStep) {
      if (skip) setDraft({ ...draft, [step]: null });
      moveBy(1);
      return;
    }
    const answers = questions.map((_, index) => index === step ? answer : answerAt(index));
    const approved = structured || answer !== null;
    sending.current = true;
    setBusy(true);
    setError(null);
    try {
      await api("respond_approval", {conversationId: request.conversationId, approvalId: request.payload.approvalId,
        response: {approved, answer: structured ? JSON.stringify(answers) : answer}});
      setSubmitted(approved ? "已提交回答" : "已跳过");
      // A refresh failure does not mean the response can be sent a second time.
      await replayConversation(request.conversationId);
    } catch (failure) { setError(failure); }
    finally { sending.current = false; setBusy(false); }
  }

  if (!question) return null;
  return <Card role="region" aria-labelledby={`${id}-title`} className="w-full min-w-0 gap-0 py-0">
    <CardHeader className="px-4 py-3.5 sm:px-5">
      <CardTitle id={`${id}-title`} className="flex items-start gap-2 leading-snug">
        <ClipboardListIcon aria-hidden="true" className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <span className="min-w-0 wrap-anywhere">{title}</span>
      </CardTitle>
    </CardHeader>
    <Separator />
    {complete ? <CardContent className="flex flex-col gap-2 px-4 py-3 text-muted-foreground sm:px-5">
      {!structured && <p className="wrap-anywhere whitespace-pre-line">{question.question}</p>}
      <p role="status">{complete}</p>
      <ErrorState error={error} />
    </CardContent> : <>
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
              className="flex min-h-10 min-w-0 cursor-pointer items-center gap-2.5 rounded-xl border p-2.5 leading-snug hover:bg-muted/60 has-[[data-state=checked]]:border-ring has-[[data-state=checked]]:bg-accent/50">
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
      <Separator />
      <CardFooter className="flex flex-wrap justify-between gap-2 px-4 py-2.5 sm:px-5">
        <div className="flex flex-wrap items-center gap-1">
          {step > 0 && <Button type="button" size="sm" variant="ghost" disabled={busy} onClick={() => moveBy(-1)}>
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
    </>}
  </Card>;
}
