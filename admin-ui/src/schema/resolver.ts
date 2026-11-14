import { standardSchemaResolver } from "@hookform/resolvers/standard-schema";
import type { FieldErrors, Resolver } from "react-hook-form";
import type { ComponentSchema, InputSchema } from "@/api/types";
import { formConditions, hasConditions, hiddenFields } from "./conditions";
import { type FormValues, wireValues, zodSchema } from "./form";

/**
 * Client rules as a resolver. On edit only changed fields are submitted, so only
 * they are checked: an untouched field (blank when the user may not view the
 * record, or a legacy value) never blocks saving the others. Inputs hidden by
 * their conditions (`components`, ADR 0037) are never checked: never sent.
 */
export function editResolver(
  inputs: InputSchema[],
  editing: boolean,
  isDirty: (name: string) => boolean,
  components?: ComponentSchema[],
): Resolver<FormValues> {
  const base = standardSchemaResolver(zodSchema(inputs)) as Resolver<FormValues>;
  const conditions = components && hasConditions(components) ? formConditions(components) : null;
  return async (values, context, options) => {
    const result = await base(values, context, options);
    if (!result.errors || (!editing && !conditions)) return result;
    const hidden = conditions ? hiddenFields(conditions, wireValues(inputs, values)) : new Set<string>();
    const errors = Object.fromEntries(
      Object.entries(result.errors).filter(([name]) => !hidden.has(name) && (!editing || isDirty(name))),
    );
    return Object.keys(errors).length ? { values: {}, errors: errors as FieldErrors<FormValues> } : { values, errors: {} };
  };
}
