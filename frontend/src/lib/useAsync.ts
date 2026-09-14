"use client";

import { useCallback, useEffect, useState } from "react";
import { ApiError } from "./api";

interface AsyncState<T> {
  data: T | null;
  error: string | null;
  loading: boolean;
  /** Re-runs the fetcher, keeping the previous data visible until it resolves. */
  reload: () => void;
}

/**
 * Runs an async fetcher on mount and whenever `deps` change. Meant for
 * simple read-only GETs; mutations (create/update/delete) call the API
 * client directly and then `reload()` the relevant list.
 */
export function useAsync<T>(fetcher: () => Promise<T>, deps: unknown[]): AsyncState<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [generation, setGeneration] = useState(0);

  const reload = useCallback(() => setGeneration((g) => g + 1), []);

  useEffect(() => {
    let cancelled = false;
    // Synchronizing with an external system (the API), matching the
    // "fetch in an effect" pattern from
    // https://react.dev/learn/you-might-not-need-an-effect#fetching-data.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setLoading(true);
    setError(null);
    fetcher()
      .then((result) => {
        if (!cancelled) setData(result);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof ApiError ? e.message : "Something went wrong.");
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, generation]);

  return { data, error, loading, reload };
}
