import { useLayoutEffect, useRef, useState } from "react";
import { notify } from "@/notifications/service";
import { useTimeout } from "./useTimers";

export function useClipboardFeedback<T>(errorSource: string) {
  const [copied, setCopied] = useState<T | null>(null);
  const generation = useRef(0);
  const { schedule, cancel } = useTimeout();

  useLayoutEffect(() => () => {
    generation.current++;
  }, []);

  const clear = () => {
    generation.current++;
    cancel();
    setCopied(null);
  };

  const copy = async (key: T, readText: () => string | Promise<string>) => {
    const request = ++generation.current;
    try {
      const text = readText();
      const value = typeof text === "string" ? text : await text;
      if (generation.current !== request) return;
      await navigator.clipboard.writeText(value);
      if (generation.current !== request) return;
      setCopied(key);
      schedule(() => setCopied(null), 1500);
    } catch (error) {
      if (generation.current === request) notify.error(error, errorSource);
    }
  };

  return { copied, copy, clear };
}
