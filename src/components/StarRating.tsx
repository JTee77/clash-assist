import { useI18n } from "../i18n";
import { useId } from "react";
import type { NodeStars } from "../lib/score/types";

type Slot = "full" | "half" | "empty";

const STAR_PATH =
  "M12 2.2l2.9 5.9 6.5.9-4.7 4.6 1.1 6.5L12 16.9 6.2 20.1l1.1-6.5L2.6 9l6.5-.9L12 2.2z";

function slotsFor(stars: Exclude<NodeStars, "unavailable">): Slot[] {
  const full = Math.floor(stars);
  const half = stars - full >= 0.5;
  const empty = 5 - full - (half ? 1 : 0);
  return [
    ...Array<Slot>(full).fill("full"),
    ...(half ? (["half"] as Slot[]) : []),
    ...Array<Slot>(empty).fill("empty"),
  ];
}

/** SVG stars so half-stars render on macOS (no rare Unicode glyphs). */
export function StarRating({
  stars,
  size = 14,
}: {
  stars: NodeStars;
  size?: number;
}) {
  const clipId = useId().replace(/:/g, "");
  const { t } = useI18n();
  if (stars === "unavailable") {
    return (
      <span className="star-rating star-rating-na" title={t("star.unavailable")}>
        {t("star.unavailable")}
      </span>
    );
  }
  const title = t("star.count", { n: stars });
  return (
    <span
      className="star-rating"
      title={title}
      aria-label={title}
      style={{ ["--star-size" as string]: `${size}px` }}
    >
      {slotsFor(stars).map((slot, i) => {
        if (slot === "half") {
          const id = `${clipId}-h${i}`;
          return (
            <svg
              key={i}
              className="star-glyph"
              viewBox="0 0 24 24"
              aria-hidden="true"
            >
              <defs>
                <clipPath id={id}>
                  <rect x="0" y="0" width="12" height="24" />
                </clipPath>
              </defs>
              <path d={STAR_PATH} className="star-empty" />
              <path
                d={STAR_PATH}
                className="star-full"
                clipPath={`url(#${id})`}
              />
            </svg>
          );
        }
        return (
          <svg
            key={i}
            className="star-glyph"
            viewBox="0 0 24 24"
            aria-hidden="true"
          >
            <path
              d={STAR_PATH}
              className={slot === "full" ? "star-full" : "star-empty"}
            />
          </svg>
        );
      })}
    </span>
  );
}
