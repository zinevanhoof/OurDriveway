<script setup lang="ts">
import {
    Drawer,
    DrawerClose,
    DrawerContent,
    DrawerTrigger,
} from '@/components/ui/drawer'
import { Calendar } from '@/components/ui/calendar'
import {
    Popover,
    PopoverContent,
    PopoverTrigger,
} from '@/components/ui/popover'
import { computed, ref, watch } from 'vue'
import FilterChips from './FilterChips.vue'
import { cn } from '@/lib/utils';
import type { TimeSlot } from '@/types/domain/spot'
import type { SpotFilter } from '@/types/SpotFilter'
import type { DateValue } from '@internationalized/date'
import { DateFormatter, getLocalTimeZone, today } from '@internationalized/date'
import { CalendarIcon, Plus, X } from '@lucide/vue'
import { toast } from 'vue-sonner'
import Button from '../ui/button/Button.vue'
import Input from '../ui/input/Input.vue'
import { Text, Title } from '@/components/base/text'
import { SectionHeader } from '@/components/base/section-header'

// Controlled open so the parent's summary pill can open the same drawer as the icon trigger.
const open = defineModel<boolean>('open')
const emit = defineEmits<{ update: [summary: string]; apply: [filter: SpotFilter] }>()

// getDay() index (0 = Sunday) → availability weekday key, so the matcher can fall
// back to a spot's recurring hours for a date without specific slots.
const WEEKDAY_KEYS = ['sunday', 'monday', 'tuesday', 'wednesday', 'thursday', 'friday', 'saturday']

const DEFAULT_START = '09:00'
const DEFAULT_END = '17:00'
const blankSlot = (): TimeSlot => ({ start: DEFAULT_START, end: DEFAULT_END })

const specificDateTimeslots = ref<Record<string, TimeSlot[]>>({})

const defaultPlaceholder = today(getLocalTimeZone())
const minDate = defaultPlaceholder

const dfShort = new DateFormatter('en-US', { month: 'short', day: 'numeric' })

const selectedDates = ref<DateValue[]>([])
const activeKey = ref('')
// ISO "YYYY-MM-DD" keys sort chronologically as plain strings.
const sortedDates = computed(() => [...selectedDates.value].sort((a, b) => a.toString().localeCompare(b.toString())))
const activeDate = computed(() => selectedDates.value.find((d) => d.toString() === activeKey.value))

// Selection is the source of truth (independent of slots). reka-ui multiple-mode
// hands back undefined once the last date is cleared, so normalize to an array;
// a freshly-added date (appended last) becomes the active one to edit.
function onDatesChange(v: DateValue | DateValue[] | undefined) {
    const next = Array.isArray(v) ? v : v ? [v] : []
    if (next.length > selectedDates.value.length) {
        activeKey.value = next[next.length - 1].toString()
    }
    selectedDates.value = next
}
// Drop slot entries whose date left the selection, and keep `active` on a live key (falls back to the last).
watch(selectedDates, () => {
    const keys = sortedDates.value.map((d) => d.toString())
    const valid = new Set(keys)
    for (const k of Object.keys(specificDateTimeslots.value)) if (!valid.has(k)) delete specificDateTimeslots.value[k]
    if (!valid.has(activeKey.value)) activeKey.value = keys[keys.length - 1] ?? ''
})

const daysLabel = computed(() => {
    const n = selectedDates.value.length
    return n ? `${n} date${n > 1 ? 's' : ''}` : 'Pick a date'
})

// Compact summary of the filter for the parent's pill button.
// Empty string = no filter (parent hides the pill).
const filterSummary = computed(() => {
    const dates = sortedDates.value
    if (!dates.length) return ''
    if (dates.length > 1) return `${dates.length} dates`
    const label = dfShort.format(dates[0].toDate(getLocalTimeZone()))
    const s = specificDateTimeslots.value[dates[0].toString()] ?? []
    if (!s.length) return label
    if (s.length === 1) return `${label} · ${s[0].start}-${s[0].end}`
    return `${label} · ${s.length} slots`
})
watch(filterSummary, (s) => emit('update', s), { immediate: true })

// Structured filter for the spot query (mirrors filterSummary).
function buildFilter(): SpotFilter {
    const single: SpotFilter['single'] = {}
    for (const d of selectedDates.value) {
        const key = d.toString()
        const weekday = WEEKDAY_KEYS[d.toDate(getLocalTimeZone()).getDay()]
        single[key] = { weekday, slots: specificDateTimeslots.value[key] ?? [] }
    }
    return { single }
}
// Apply on close (Apply button and swipe both close the drawer) — not on every slot edit.
watch(open, (o, prev) => {
    if (prev && !o) emit('apply', buildFilter())
})

// { key, label } chips drive the shared FilterChips component.
const dateChips = computed(() =>
    sortedDates.value.map((d) => ({ key: d.toString(), label: dfShort.format(d.toDate(getLocalTimeZone())) })),
)

// The slot list for the active date.
const slots = computed<TimeSlot[]>(() => specificDateTimeslots.value[activeKey.value] ?? [])

// Greyed when nothing is selected to attach slots to.
const addDisabled = computed(() => !selectedDates.value.length)

function addSlot() {
    if (addDisabled.value) {
        toast.warning('Pick a date first')
        return
    }
    (specificDateTimeslots.value[activeKey.value] ??= []).push(blankSlot())
}

function removeSlot(i: number) {
    slots.value.splice(i, 1)
}

function removeDate(key: string) {
    selectedDates.value = selectedDates.value.filter((d) => d.toString() !== key)
}

function reset() {
    selectedDates.value = []
    activeKey.value = ''
    specificDateTimeslots.value = {}
}
</script>

<template>
    <Drawer v-model:open="open">
        <DrawerTrigger as-child>
            <slot></slot>
        </DrawerTrigger>
        <DrawerContent class="data-[vaul-drawer-direction=bottom]:mb-[calc(3.75rem+var(--safe-bottom))]">
            <div class="m-4 space-y-4">
                <SectionHeader>
                    <Title size="xl" weight="extrabold">Filters</Title>
                    <template #action>
                        <button type="button" @click="reset" class="text-primary font-semibold">Reset</button>
                    </template>
                </SectionHeader>
                <div class="space-y-1">
                    <SectionHeader>
                        <Title>Days</Title>
                        <template #action>
                            <Text size="md" weight="semibold">{{ daysLabel }}</Text>
                        </template>
                    </SectionHeader>
                    <Popover>
                        <PopoverTrigger as-child>
                            <Button variant="outline"
                                :class="cn('w-full justify-start', !selectedDates.length && 'text-muted-foreground')">
                                <CalendarIcon />
                                {{ daysLabel }}
                            </Button>
                        </PopoverTrigger>
                        <PopoverContent class="w-auto p-0" align="start" :collision-padding="{ bottom: 60 }">
                            <Calendar multiple :model-value="(selectedDates as any)" @update:model-value="onDatesChange"
                                :default-placeholder="defaultPlaceholder" :min-value="minDate" layout="month-and-year"
                                initial-focus />
                        </PopoverContent>
                    </Popover>
                    <FilterChips :items="dateChips" :active="activeKey" @select="activeKey = $event"
                        @remove="removeDate" />
                </div>
                <div class="space-y-1">
                    <SectionHeader>
                        <Title>
                            Time slots<template v-if="activeDate"> · {{
                                dfShort.format(activeDate.toDate(getLocalTimeZone())) }}</template>
                        </Title>
                        <template #action>
                            <button type="button" @click="addSlot"
                                :class="cn('flex gap-1 items-center font-bold cursor-pointer', addDisabled ? 'text-muted-foreground' : 'text-primary')">
                                <Plus :size="16" />
                                Add slot
                            </button>
                        </template>
                    </SectionHeader>
                    <div v-auto-animate class="space-y-2 max-h-60 overflow-y-auto no-scrollbar">
                        <div v-for="(slot, i) in slots" :key="i" class="flex items-center gap-2">
                            <Input type="time" required :model-value="slot.start"
                                @update:model-value="(v) => slot.start = (v as string) || DEFAULT_START" />
                            <div>to</div>
                            <Input type="time" required :model-value="slot.end"
                                @update:model-value="(v) => slot.end = (v as string) || DEFAULT_END" />
                            <Button variant="outline" size="icon" @click="removeSlot(i)"
                                class="bg-accent text-accent-foreground">
                                <X :size="16" />
                            </Button>
                        </div>
                    </div>
                </div>
                <DrawerClose as-child>
                    <Button class="w-full h-11 font-bold">
                        {{ filterSummary ? 'Apply filters' : 'Show all spots' }}
                    </Button>
                </DrawerClose>
            </div>
        </DrawerContent>
    </Drawer>
</template>
