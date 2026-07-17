let cachedHasSelectorSupport: boolean | undefined;

export const supportsHasSelector = (): boolean => {
  if (typeof cachedHasSelectorSupport === "boolean") {
    return cachedHasSelectorSupport;
  }

  if (
    typeof window === "undefined" ||
    typeof CSS === "undefined" ||
    typeof CSS.supports !== "function"
  ) {
    cachedHasSelectorSupport = false;
    return cachedHasSelectorSupport;
  }

  try {
    cachedHasSelectorSupport = CSS.supports("selector(:has(*))");
  } catch {
    cachedHasSelectorSupport = false;
  }

  return cachedHasSelectorSupport;
};

export const isLegacyHasSelectorBrowser = (): boolean => {
  return !supportsHasSelector();
};
