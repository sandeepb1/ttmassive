// The kit's class helpers. tailwind-merge needs to know our custom scale names (see tailwind.css @theme),
// otherwise it reads `text-md` as a colour and `h-ctl` as unknown, and merging drops the wrong class.

import { extendTailwindMerge } from 'tailwind-merge';
import { createTV } from 'tailwind-variants';

const twMergeConfig = {
  extend: {
    theme: {
      text: ['2xs', 'md', 'touch'],
      spacing: ['ctl', 'ctl-sm', 'ctl-xs', 'touch'],
      shadow: ['pop'],
    },
  },
};

const merge = extendTailwindMerge(twMergeConfig);

export const tv = createTV({ twMergeConfig });

/** Join class names, later ones winning conflicts. */
export function cn(...parts: (string | false | null | undefined)[]): string {
  return merge(parts.filter(Boolean).join(' '));
}
