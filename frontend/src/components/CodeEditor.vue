<script setup lang="ts">
import { onMounted, onUnmounted, ref, watch } from 'vue'
import type { Extension } from '@codemirror/state'
import type { EditorView } from '@codemirror/view'
import { useTheme } from '@/app/theme'

type EditorLanguage = 'python' | 'shell' | 'rust' | 'json' | 'text'

const props = withDefaults(defineProps<{
  id?: string
  modelValue: string
  language?: EditorLanguage
  disabled?: boolean
  placeholder?: string
  ariaLabel?: string
}>(), {
  language: 'python',
  disabled: false,
  placeholder: '',
})
const emit = defineEmits<{ 'update:modelValue': [value: string] }>()

const host = ref<HTMLDivElement>()
const editorReady = ref(false)
const loadingError = ref('')
const languageError = ref('')
const { theme } = useTheme()
let editor: EditorView | undefined
let alive = true
let languageRevision = 0
let languageCompartment: import('@codemirror/state').Compartment | undefined
let editableCompartment: import('@codemirror/state').Compartment | undefined
let placeholderCompartment: import('@codemirror/state').Compartment | undefined
let themeCompartment: import('@codemirror/state').Compartment | undefined
let keymapCompartment: import('@codemirror/state').Compartment | undefined
let lineSeparatorCompartment: import('@codemirror/state').Compartment | undefined
let mountedImports: {
  EditorState: typeof import('@codemirror/state').EditorState
  EditorView: typeof import('@codemirror/view').EditorView
  Compartment: typeof import('@codemirror/state').Compartment
  keymap: typeof import('@codemirror/view').keymap
  lineNumbers: typeof import('@codemirror/view').lineNumbers
  highlightActiveLine: typeof import('@codemirror/view').highlightActiveLine
  highlightActiveLineGutter: typeof import('@codemirror/view').highlightActiveLineGutter
  bracketMatching: typeof import('@codemirror/language').bracketMatching
  indentOnInput: typeof import('@codemirror/language').indentOnInput
  search: typeof import('@codemirror/search').search
  searchKeymap: typeof import('@codemirror/search').searchKeymap
  highlightSelectionMatches: typeof import('@codemirror/search').highlightSelectionMatches
  indentWithTab: typeof import('@codemirror/commands').indentWithTab
  defaultKeymap: typeof import('@codemirror/commands').defaultKeymap
  history: typeof import('@codemirror/commands').history
  historyKeymap: typeof import('@codemirror/commands').historyKeymap
  temporarilySetTabFocusMode: typeof import('@codemirror/commands').temporarilySetTabFocusMode
  simplifySelection: typeof import('@codemirror/commands').simplifySelection
  placeholder: typeof import('@codemirror/view').placeholder
  HighlightStyle: typeof import('@codemirror/language').HighlightStyle
  syntaxHighlighting: typeof import('@codemirror/language').syntaxHighlighting
  tags: typeof import('@lezer/highlight').tags
} | undefined

async function languageExtension(language: EditorLanguage): Promise<Extension> {
  switch (language) {
    case 'python': return (await import('@codemirror/lang-python')).python()
    case 'rust': return (await import('@codemirror/lang-rust')).rust()
    case 'json': return (await import('@codemirror/lang-json')).json()
    case 'shell': {
      const [{ StreamLanguage }, { shell }] = await Promise.all([
        import('@codemirror/language'),
        import('@codemirror/legacy-modes/mode/shell'),
      ])
      return StreamLanguage.define(shell)
    }
    case 'text': return []
  }
}

function lineSeparatorFor(value: string): string {
  const firstBreak = value.match(/\r\n|\n|\r/)
  return firstBreak?.[0] ?? '\n'
}

function themeExtensions(): Extension[] {
  const modules = mountedImports
  if (!modules) return []
  const oled = theme.value === 'oled'
  const tags = modules.tags
  const highlights = modules.HighlightStyle.define([
    { tag: tags.comment, color: oled ? '#858585' : '#929292', fontStyle: 'italic' },
    { tag: [tags.keyword, tags.modifier, tags.operatorKeyword], color: oled ? '#d2a8ff' : '#c4a1ff' },
    { tag: [tags.string, tags.character, tags.special(tags.string)], color: oled ? '#a5d6a7' : '#a8dba8' },
    { tag: [tags.number, tags.integer, tags.float], color: oled ? '#f2cc60' : '#e9c46a' },
    { tag: [tags.bool, tags.null], color: oled ? '#ff9bce' : '#f08cbd' },
    { tag: [tags.definition(tags.variableName), tags.function(tags.variableName)], color: oled ? '#79c0ff' : '#83b7f2' },
    { tag: [tags.typeName, tags.className, tags.namespace], color: oled ? '#ffa657' : '#e8a56a' },
  ])
  const editorTheme = modules.EditorView.theme({
    '&': {
      color: 'var(--foreground)',
      backgroundColor: 'var(--background)',
      border: '1px solid var(--border)',
      borderRadius: '0.375rem',
      fontSize: '0.8125rem',
    },
    '&.cm-focused': { outline: '2px solid var(--ring)', outlineOffset: '1px' },
    '.cm-scroller': { maxHeight: '18rem', minHeight: '12rem', overflow: 'auto', fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace', lineHeight: '1.5' },
    '.cm-content': { minHeight: '12rem', padding: '0.625rem 0' },
    '.cm-line': { padding: '0 0.75rem' },
    '.cm-gutters': { color: 'var(--muted-foreground)', backgroundColor: 'var(--background)', borderRight: '1px solid var(--border)' },
    '.cm-activeLine': { backgroundColor: 'color-mix(in srgb, var(--secondary) 55%, transparent)' },
    '.cm-activeLineGutter': { color: 'var(--foreground)', backgroundColor: 'var(--secondary)' },
    '.cm-selectionBackground, &.cm-focused .cm-selectionBackground': { backgroundColor: oled ? '#343434' : '#343d4d' },
    '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--foreground)' },
    '.cm-matchingBracket, .cm-nonmatchingBracket': { backgroundColor: 'var(--secondary)', outline: '1px solid var(--ring)' },
    '.cm-tooltip': { color: 'var(--foreground)', backgroundColor: 'var(--card)', border: '1px solid var(--border)' },
    '.cm-search': { padding: '0.375rem', backgroundColor: 'var(--card)' },
    '.cm-search input': { color: 'var(--foreground)', backgroundColor: 'var(--background)', border: '1px solid var(--border)' },
  }, { dark: true })
  return [editorTheme, modules.syntaxHighlighting(highlights, { fallback: true })]
}

function syncAttributes() {
  if (!editor) return
  const content = editor.contentDOM
  if (props.id) content.id = props.id
  else content.removeAttribute('id')
  content.setAttribute('role', 'textbox')
  content.setAttribute('aria-multiline', 'true')
  content.setAttribute('aria-disabled', String(props.disabled))
  content.setAttribute('aria-label', props.ariaLabel ?? 'Code editor')
  content.setAttribute('aria-readonly', String(props.disabled))
}

watch(() => props.modelValue, (value) => {
  if (!editor || !lineSeparatorCompartment || !mountedImports) return
  const separator = lineSeparatorFor(value)
  if (editor.state.lineBreak !== separator) {
    editor.dispatch({ effects: lineSeparatorCompartment.reconfigure(mountedImports.EditorState.lineSeparator.of(separator)) })
  }
  const current = editor.state.sliceDoc()
  if (current !== value) editor.dispatch({ changes: { from: 0, to: editor.state.doc.length, insert: value } })
})
watch(() => props.disabled, (disabled) => {
  if (!editor || !editableCompartment || !keymapCompartment || !mountedImports) return
  editor.dispatch({ effects: [
    editableCompartment.reconfigure([
      mountedImports.EditorView.editable.of(!disabled),
      mountedImports.EditorState.readOnly.of(disabled),
    ]),
    keymapCompartment.reconfigure(disabled ? [] : mountedImports.keymap.of([
      { key: 'Escape', run: (view) => view.dom.querySelector('.cm-search') ? false : view.state.selection.main.empty ? mountedImports!.temporarilySetTabFocusMode(view) : mountedImports!.simplifySelection(view) },
      mountedImports.indentWithTab,
    ])),
  ] })
  syncAttributes()
})
watch(() => props.placeholder, (value) => {
  if (!editor || !placeholderCompartment || !mountedImports) return
  editor.dispatch({ effects: placeholderCompartment.reconfigure(value ? mountedImports.placeholder(value) : []) })
})
watch(() => [props.id, props.ariaLabel] as const, syncAttributes)
watch(() => props.language, (value) => {
  const revision = ++languageRevision
  if (!editor || !languageCompartment) return
  void applyLanguage(value, revision)
})
watch(theme, () => {
  if (editor && themeCompartment) editor.dispatch({ effects: themeCompartment.reconfigure(themeExtensions()) })
})

async function applyLanguage(value: EditorLanguage, revision: number): Promise<void> {
  try {
    const language = await languageExtension(value)
    if (alive && editor && languageCompartment && revision === languageRevision) {
      editor.dispatch({ effects: languageCompartment.reconfigure(language) })
      languageError.value = ''
    }
  } catch (error) {
    if (alive && revision === languageRevision) languageError.value = error instanceof Error ? error.message : 'Language highlighting could not be loaded.'
  }
}

onMounted(async () => {
  try {
    const [state, view, commands, language, search, highlight] = await Promise.all([
      import('@codemirror/state'), import('@codemirror/view'), import('@codemirror/commands'),
      import('@codemirror/language'), import('@codemirror/search'), import('@lezer/highlight'),
    ])
    if (!alive || !host.value) return
    mountedImports = { ...state, ...view, ...commands, ...language, ...search, ...highlight }
    languageCompartment = new state.Compartment()
    editableCompartment = new state.Compartment()
    placeholderCompartment = new state.Compartment()
    themeCompartment = new state.Compartment()
    keymapCompartment = new state.Compartment()
    lineSeparatorCompartment = new state.Compartment()
    const startingLanguage = props.language
    const startingLanguageRevision = languageRevision
    const initialLanguage = await languageExtension(startingLanguage)
    if (!alive || !host.value) return
    editor = new view.EditorView({
      state: state.EditorState.create({
        doc: props.modelValue,
        extensions: [
          keymapCompartment.of(props.disabled ? [] : view.keymap.of([
            { key: 'Escape', run: (view) => view.dom.querySelector('.cm-search') ? false : view.state.selection.main.empty ? commands.temporarilySetTabFocusMode(view) : commands.simplifySelection(view) },
            commands.indentWithTab,
          ])),
          view.lineNumbers(),
          view.highlightActiveLine(),
          view.highlightActiveLineGutter(),
          language.bracketMatching(),
          language.indentOnInput(),
          commands.history(),
          search.search(),
          search.highlightSelectionMatches(),
          view.keymap.of([...search.searchKeymap, ...commands.historyKeymap, ...commands.defaultKeymap]),
          languageCompartment.of(initialLanguage),
          editableCompartment.of([
            view.EditorView.editable.of(!props.disabled),
            state.EditorState.readOnly.of(props.disabled),
          ]),
          lineSeparatorCompartment.of(state.EditorState.lineSeparator.of(lineSeparatorFor(props.modelValue))),
          placeholderCompartment.of(props.placeholder ? view.placeholder(props.placeholder) : []),
          themeCompartment.of(themeExtensions()),
          state.EditorState.tabSize.of(4),
          view.EditorView.updateListener.of((update) => {
            if (update.docChanged) emit('update:modelValue', update.state.sliceDoc())
          }),
        ],
      }),
      parent: host.value,
    })
    syncAttributes()
    editorReady.value = true
    if (startingLanguage !== props.language || startingLanguageRevision !== languageRevision) {
      void applyLanguage(props.language, languageRevision)
    }
  } catch (error) {
    if (alive) loadingError.value = error instanceof Error ? error.message : 'Editor could not be loaded.'
  }
})

onUnmounted(() => {
  alive = false
  editor?.destroy()
  editor = undefined
})
</script>

<template>
  <div class="code-editor min-h-48 w-full max-w-full" :data-language="language">
    <div v-if="loadingError" role="alert" class="rounded border border-destructive p-3 text-sm text-destructive">{{ loadingError }}</div>
    <div v-else ref="host" class="min-h-48 w-full" :aria-busy="!editorReady" />
    <p v-if="languageError" role="alert" class="mt-1 text-xs text-destructive">Syntax highlighting is unavailable: {{ languageError }}</p>
  </div>
</template>

<style scoped>
.code-editor :deep(.cm-editor) { width: 100%; max-width: 100%; }
.code-editor :deep(.cm-placeholder) { color: var(--muted-foreground); }
.code-editor :deep(.cm-searchMatch) { background-color: color-mix(in srgb, var(--ring) 35%, transparent); }
.code-editor :deep(.cm-selectionMatch) { background-color: color-mix(in srgb, var(--ring) 24%, transparent); }
</style>
