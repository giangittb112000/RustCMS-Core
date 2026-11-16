// Form reactions (ADR 0041): after an input with `reacts` changes and typing
// pauses, the form's values go to the server's reaction; its answer is
// applied only if it is the latest one and the form has not changed since
// (a newer request is then on its way). Failures are ignored: the form works
// without reactions, and the save is checked anyway.
import { useCallback, useEffect, useRef, useState } from "react";
import type { UseFormReturn } from "react-hook-form";
import { pushToast } from "@/lib/toast";
import type { FormReactRequest, FormReactResponse, InputSchema } from "./types";
import { useApi } from "./context";
import type { FormValues } from "@/schema/form";
import { REACTION_DELAY_MS, type RowFlags, applyPatches, reactionState } from "@/schema/reactions";

const enc = encodeURIComponent;

export interface FormReactions {
  /** Call on every change the user makes (any input). */
  onUserChange: (name: string) => void;
  /** Warnings under rows, from the latest answer. */
  flags: RowFlags;
}

export function useFormReactions({ slug, recordKey, inputs, form, paused, onPatched }: {
  slug: string;
  /** The edited record (absent: a create form). */
  recordKey?: string;
  inputs: InputSchema[];
  form: UseFormReturn<FormValues>;
  /** While true (saving, read-only), answers are not applied. */
  paused: boolean;
  /** An input the answer changed (a create form then sends it). */
  onPatched: (name: string) => void;
}): FormReactions {
  const api = useApi();
  const [flags, setFlags] = useState<RowFlags>({});
  // Changes the user made so far, and the latest request.
  const changes = useRef(0);
  const seq = useRef(0);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const live = useRef(true);
  const latest = useRef({ paused, onPatched, inputs });
  latest.current = { paused, onPatched, inputs };
  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
      if (timer.current) clearTimeout(timer.current);
    };
  }, []);

  const ask = useCallback(
    async (changed: string) => {
      const sent = ++seq.current;
      const before = changes.current;
      // An answer from an earlier session (sign-out, another user) is never applied.
      const epoch = api.epoch;
      const request: FormReactRequest = {
        operation: recordKey === undefined ? "create" : "edit",
        ...(recordKey === undefined ? {} : { key: recordKey }),
        changed,
        state: reactionState(latest.current.inputs, form.getValues()),
        seq: sent,
      };
      let answer: FormReactResponse;
      try {
        answer = await api.post<FormReactResponse>(`/resources/${enc(slug)}/form/react`, request);
      } catch {
        return; // network, 429, refused: no help this time
      }
      const { paused, onPatched, inputs } = latest.current;
      if (!live.current || api.epoch !== epoch || answer.seq !== seq.current || changes.current !== before || paused || answer.failed) return;
      const applied = applyPatches(inputs, form.getValues(), answer.patches);
      for (const [name, value] of Object.entries(applied.values)) {
        form.setValue(name, value, { shouldDirty: true });
        onPatched(name);
      }
      setFlags(applied.flags);
      for (const notice of applied.notices) pushToast("success", notice, epoch);
    },
    [api, form, recordKey, slug],
  );

  const onUserChange = useCallback(
    (name: string) => {
      changes.current += 1;
      if (!latest.current.inputs.some((i) => i.name === name && i.reacts && !i.disabled)) return;
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => {
        timer.current = null;
        void ask(name);
      }, REACTION_DELAY_MS);
    },
    [ask],
  );

  return { onUserChange, flags };
}
