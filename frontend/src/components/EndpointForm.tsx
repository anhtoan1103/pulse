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

export function EndpointForm({
  initial,
  submitLabel,
  onSubmit,
}: {
  initial?: Endpoint;
  submitLabel: string;
  onSubmit: (input: EndpointInput) => Promise<void>;
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
      await onSubmit({
        name,
        url,
        method,
        check_interval_seconds: interval,
        latency_threshold_ms: latency,
        error_rate_threshold_percent: errorRate,
      });
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
