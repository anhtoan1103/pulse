"use client";

import { useState, FormEvent } from "react";
import type { Endpoint, EndpointInput } from "@/lib/api";
import { Button, ErrorBanner, Field, Input, Select } from "@/components/ui";

const METHODS = ["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"];

// Mirrors backend/src/endpoints/validate.rs.
const LIMITS = {
  interval: { min: 10, max: 3600 },
  latency: { min: 1, max: 10_000 },
  errorRate: { min: 0, max: 100 },
};

/** Only the fields of `values` that differ from `initial`'s own value. */
function diffFromInitial(values: EndpointInput, initial: Endpoint): Partial<EndpointInput> {
  const changed: Partial<EndpointInput> = {};
  if (values.name !== initial.name) changed.name = values.name;
  if (values.url !== initial.url) changed.url = values.url;
  if (values.method !== initial.method) changed.method = values.method;
  if (values.check_interval_seconds !== initial.check_interval_seconds) {
    changed.check_interval_seconds = values.check_interval_seconds;
  }
  if (values.latency_threshold_ms !== initial.latency_threshold_ms) {
    changed.latency_threshold_ms = values.latency_threshold_ms;
  }
  if (values.error_rate_threshold_percent !== initial.error_rate_threshold_percent) {
    changed.error_rate_threshold_percent = values.error_rate_threshold_percent;
  }
  return changed;
}

export function EndpointForm({
  initial,
  submitLabel,
  onSubmit,
}: {
  initial?: Endpoint;
  submitLabel: string;
  /**
   * Without `initial` (create), always called with every field. With
   * `initial` (edit), called with only the fields that actually changed —
   * possibly `{}` if the user hit save without changing anything.
   */
  onSubmit: (input: Partial<EndpointInput>) => Promise<void>;
}) {
  const [name, setName] = useState(initial?.name ?? "");
  const [url, setUrl] = useState(initial?.url ?? "");
  const [method, setMethod] = useState(initial?.method ?? "GET");
  const [interval, setInterval] = useState(initial?.check_interval_seconds ?? 60);
  const [latency, setLatency] = useState(initial?.latency_threshold_ms ?? 2000);
  const [errorRate, setErrorRate] = useState(initial?.error_rate_threshold_percent ?? 5);
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    setError(null);
    setSubmitting(true);
    try {
      const values: EndpointInput = {
        name,
        url,
        method,
        check_interval_seconds: interval,
        latency_threshold_ms: latency,
        error_rate_threshold_percent: errorRate,
      };
      // Editing: send only what actually changed. The backend treats an
      // omitted url as "leave it alone" and skips its SSRF/DNS re-check —
      // always resending it would re-run that check on every save (even a
      // pure rename), so a transient DNS hiccup on the target could reject
      // an edit that never touched the URL at all.
      const input = initial ? diffFromInitial(values, initial) : values;
      await onSubmit(input);
    } catch (err) {
      setError(err instanceof Error ? err.message : "Something went wrong.");
      setSubmitting(false);
    }
  }

  return (
    <form className="space-y-4" onSubmit={handleSubmit}>
      <ErrorBanner message={error} />

      <Field label="Name">
        <Input required maxLength={100} value={name} onChange={(e) => setName(e.target.value)} placeholder="Orders API" />
      </Field>

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-[1fr_auto]">
        <Field label="URL">
          <Input
            required
            type="url"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            placeholder="https://api.example.com/health"
          />
        </Field>
        <Field label="Method">
          <Select value={method} onChange={(e) => setMethod(e.target.value)}>
            {METHODS.map((m) => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
          </Select>
        </Field>
      </div>

      <div className="grid grid-cols-1 gap-4 sm:grid-cols-3">
        <Field label="Check interval (s)" hint={`${LIMITS.interval.min}–${LIMITS.interval.max}`}>
          <Input
            type="number"
            required
            min={LIMITS.interval.min}
            max={LIMITS.interval.max}
            value={interval}
            onChange={(e) => setInterval(Number(e.target.value))}
          />
        </Field>
        <Field label="Latency threshold (ms)" hint={`${LIMITS.latency.min}–${LIMITS.latency.max}`}>
          <Input
            type="number"
            required
            min={LIMITS.latency.min}
            max={LIMITS.latency.max}
            value={latency}
            onChange={(e) => setLatency(Number(e.target.value))}
          />
        </Field>
        <Field label="Error rate threshold (%)" hint={`${LIMITS.errorRate.min}–${LIMITS.errorRate.max}`}>
          <Input
            type="number"
            required
            step="0.1"
            min={LIMITS.errorRate.min}
            max={LIMITS.errorRate.max}
            value={errorRate}
            onChange={(e) => setErrorRate(Number(e.target.value))}
          />
        </Field>
      </div>

      <Button type="submit" disabled={submitting}>
        {submitting ? "Saving…" : submitLabel}
      </Button>
    </form>
  );
}
