import { IGNORE_PROXY_TYPES, JUNK_NAME_KEYWORDS, type ProxyInfo } from "./types";

function isJunkName(name: string): boolean {
  if (name.startsWith("PASS") || name.startsWith("REJECT")) return true;
  return JUNK_NAME_KEYWORDS.some((k) => name.includes(k));
}

export type ProviderMap = Record<string, {
  vehicleType?: string;
  proxies?: ProxyInfo[];
}>;
const GROUP_TYPES = new Set(["Selector", "URLTest", "Fallback", "LoadBalance"]);

export function missingProviderNames(proxies: Record<string, ProxyInfo>): string[] {
  const missing = new Set<string>();
  for (const group of Object.values(proxies)) {
    if (!GROUP_TYPES.has(group.type)) continue;
    for (const name of [...(group.all ?? []), group.now]) {
      if (typeof name === "string" && name && !isJunkName(name) && !Object.prototype.hasOwnProperty.call(proxies, name)) missing.add(name);
    }
  }
  return [...missing].sort();
}

/** Compatible providers repeat group membership; prefer an actual subscription.
 * A same-name node in two subscriptions is ambiguous: never choose the first. */
export function resolveProviderNode(providers: ProviderMap, name: string): {
  provider: string;
  node: ProxyInfo;
} | null {
  const matches = Object.entries(providers).flatMap(([provider, info]) =>
    Array.isArray(info?.proxies) ? info.proxies.filter((node) =>
      node?.name === name && typeof node.type === "string" && node.type.length > 0 &&
      !IGNORE_PROXY_TYPES.has(node.type),
    ).map((node) => ({ provider, node, compatible: info.vehicleType === "Compatible" })) : [],
  );
  const subscriptions = matches.filter((m) => !m.compatible);
  const candidates = subscriptions.length ? subscriptions : matches;
  return candidates.length === 1 ? candidates[0] : null;
}

export function mergeProviderNodes(
  proxies: Record<string, ProxyInfo>, providers: ProviderMap,
): Record<string, ProxyInfo> {
  const merged = { ...proxies };
  for (const name of missingProviderNames(proxies)) {
    const found = resolveProviderNode(providers, name);
    if (found) Object.defineProperty(merged, name, { value: found.node, enumerable: true, configurable: true, writable: true });
  }
  return merged;
}
