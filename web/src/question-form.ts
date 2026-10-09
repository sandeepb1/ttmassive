// The state of an interactive question card: what is picked or typed for each question, whether
// the form is complete, and the request that sends it. Pure, so node can test it.

import type { Question } from './chat-model';

/** What has been chosen for one question. `choices` are 1-based option numbers. */
export interface FormAnswer {
  choices: number[];
  text: string;
}

export type FormState = FormAnswer[];

export interface QuestionRequest {
  answers: { choices: number[]; text: string | null }[];
}

export function initialForm(qs: Question[]): FormState {
  return qs.map(() => ({ choices: [], text: '' }));
}

/** A single question with one answer: tapping an option can send it straight away. */
export function isInstant(qs: Question[]): boolean {
  return qs.length === 1 && !qs[0].multi_select;
}

/** Pick (or, for multiple choice, toggle) option `n` of question `i`. Picking clears typed text. */
export function pickOption(qs: Question[], state: FormState, i: number, n: number): FormState {
  const q = qs[i];
  if (!q || n < 1 || n > q.options.length) return state;
  return state.map((a, k) => {
    if (k !== i) return a;
    if (!q.multi_select) return { choices: [n], text: '' };
    const has = a.choices.includes(n);
    return { choices: has ? a.choices.filter((c) => c !== n) : [...a.choices, n].sort((x, y) => x - y), text: '' };
  });
}

/** Type an answer of your own for question `i`. It replaces any picked option. */
export function typeAnswer(state: FormState, i: number, text: string): FormState {
  return state.map((a, k) => (k === i ? { choices: text.trim() ? [] : a.choices, text } : a));
}

export function isComplete(state: FormState): boolean {
  return state.length > 0 && state.every((a) => a.choices.length > 0 || a.text.trim() !== '');
}

export function toRequest(state: FormState): QuestionRequest {
  return {
    answers: state.map((a) => ({
      choices: a.text.trim() ? [] : a.choices,
      text: a.text.trim() || null,
    })),
  };
}
