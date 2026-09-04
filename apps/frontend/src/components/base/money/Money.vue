<script setup lang="ts">
import { computed } from "vue"
import type { HTMLAttributes } from "vue"
import type { TitleVariants } from "../text"
import { cn } from "@/lib/utils"
import { formatCents } from "@/lib/money"
import { Text, Title } from "../text"

/**
 * An amount, and optionally what it is per.
 *
 * The formatting is not reimplemented — `formatCents` in lib/money.ts is already shared
 * by eleven files and is the thing that knows cents are the transport format. Only the
 * *presentation* was duplicated: the price of a spot renders four different ways
 * (`text-xl font-extrabold text-primary`, `text-lg font-bold text-primary`, `text-lg
 * font-semibold`, and bare `font-extrabold text-primary`), each with the same
 * baseline-aligned muted `/hr` glued on by hand.
 *
 * No `currency` prop. lib/money.ts takes one and documents why — real conversion needs a
 * rate valid at the time of the booking — but every call site uses the default, and
 * threading it through here before that machinery exists would just be a second place to
 * update when it does.
 *
 * Known limit: `tone` colours the amount, not the suffix, which is always muted. No call
 * site puts a suffix on a filled surface today; the one that does first will want a
 * `suffixTone` rather than a `class`, since `class` reaches the amount now.
 */
const props = withDefaults(defineProps<{
  cents: number
  /** Rendered small and muted, on the value's baseline. Typically "/hr". */
  suffix?: string
  /** Prefix an explicit + or −, and colour a positive amount as success. */
  signed?: boolean
  class?: HTMLAttributes["class"]
  size?: TitleVariants["size"]
  weight?: TitleVariants["weight"]
  tone?: TitleVariants["tone"]
}>(), {
  size: "lg",
  weight: "bold",
})

// U+2212 MINUS SIGN, not a hyphen: it aligns with the digits at these weights, which is
// why WalletComponent's local helper used it.
const label = computed(() =>
  props.signed
    ? `${props.cents >= 0 ? "+" : "−"}${formatCents(Math.abs(props.cents))}`
    : formatCents(props.cents),
)

// An explicit `tone` always wins. Otherwise a signed amount colours itself, which is
// what four call sites were doing with an inline ternary.
const resolvedTone = computed<TitleVariants["tone"]>(() => {
  if (props.tone) return props.tone
  if (props.signed) return props.cents >= 0 ? "success" : "default"
  return "default"
})
</script>

<template>
  <!--
    The Title IS the flex container — there is no wrapper element.

    With a wrapper, `class` landed on the wrapper while the digits were sized by the
    Title inside it, so `class="text-[34px]"` did nothing: a font-size is only inherited
    by a child that does not set its own, and Title always sets one. Two elements also
    put the two `text-*` classes in two different `cn` calls, where tailwind-merge cannot
    see them together to resolve the conflict.

    Merged onto one element they end up in one string, last wins, and the caller can
    reach any size without a prop for it.
  -->
  <Title
    as="span"
    data-slot="money"
    :size="size"
    :weight="weight"
    :tone="resolvedTone"
    :class="cn('inline-flex items-baseline gap-0.5', props.class)"
  >
    {{ label }}
    <Text v-if="suffix" as="span">{{ suffix }}</Text>
  </Title>
</template>
