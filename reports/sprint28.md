# Sprint 28 — Fase 6: Shadow DOM e Custom Elements (subconjunto mínimo)

Escopo: Fase 6 do plano mestre, conforme o critério registrado nele — `attachShadow`,
`shadowRoot`, `customElements.define` com `connectedCallback`, `:host` e composição para
estilo e layout. Fase 7 não foi iniciada. Nenhuma correção visual do Google foi reaberta.

**Não é suporte completo a Web Components.** O que está implementado e o que não está
estão nas seções 2 e 5.

## 1. Diagnóstico antes de implementar

| Componente | Estado encontrado | Ponto de integração necessário |
|---|---|---|
| `naro-html` | `NodeData` com `Document`/`Element`/`Text`; `Document` com `nodes/root/title`; filhos em `Vec<NodeId>` | Um nó que **não** seja filho do host (senão `host.children` e todo walk de árvore passam a mentir) e uma tabela host→root |
| `naro-css` | cascata em `compute_styles_with`; `Selector::matches` sem escopo; `Pseudo` sem `:host` | Um parâmetro de escopo no matching e uma passada de folha por árvore |
| `naro-layout` | `build_children_into` percorre `doc.children(parent)` | Composição: usar os filhos do shadow root quando o host tem um |
| `naro-js` | `wrap(id)` com identidade estável por `NodeId`; `HTMLElement` inexistente | Registro de custom elements + `attachShadow`/`shadowRoot` no wrapper |
| `naro-engine` | `parse_stylesheet_for(doc.collect_style_text())` em 4 pontos | Um helper único que também parseia as folhas das árvores |

A escolha central: o shadow root é um **nó próprio**, com `parent` apontando para o host,
mas **fora** da lista de filhos. Isso dá herança de propriedades e `getRootNode()` de graça,
e mantém todo walk existente (`children`, `querySelector`, layout do light DOM) correto sem
filtro extra.

## 2. Funcionalidades implementadas

### 2.1 Modelo (`naro-html`)

- `NodeData::ShadowRoot { host, mode }` e `ShadowMode::{Open, Closed}`.
- `Document::shadow_roots: Vec<(host, root)>` — tabela lateral, não campo do `Node`.
- `attach_shadow`, `shadow_root`, `shadow_host`, `shadow_mode`, `shadow_scope`
  (árvore envolvente de um nó), `elements_by_tag` (todos os nós, inclusive em shadow
  trees, para o upgrade) e `collect_style_text_from(root)`.

### 2.2 Estilo (`naro-css`)

- `Pseudo::Host(Option<Box<Compound>>)`: `:host` e `:host(.classe)`. Conta como uma
  pseudo-classe na especificidade.
- `Selector::matches_scoped(doc, id, scope)` e `Selector::has_host_pseudo()`.
- `compute_styles_scoped(doc, author, shadow: &HashMap<NodeId, Stylesheet>, ctx)`:
  - nó **dentro** de uma árvore: UA + folha **daquela** árvore. A folha do documento não
    o alcança;
  - nó **fora**: UA + folha do documento + (se for host) as regras `:host` da sua árvore;
  - a folha de uma árvore nunca estiliza o host por outro seletor que não `:host`;
  - `:host` sem escopo nunca casa — a folha do documento não pode nomear um host;
  - variáveis CSS: as do documento entram na árvore, as da árvore vencem no conflito;
  - precedência de encapsulamento: `:host` normal perde para regra normal do documento e
    `:host !important` vence `!important` do documento (ordenação por `order`, com base
    deslocada para as declarações importantes internas).

### 2.3 Layout (`naro-layout`)

- `composed_children(doc, id)` devolve `doc.children(shadow_root)` quando existe, senão
  `doc.children(id)`. Usado por `build_children_into` e `flatten_inline`, ou seja, block,
  inline, flex, grid e tabela herdam a composição.
- O shadow root em si não gera caixa; o host assume a altura do conteúdo composto.

### 2.4 JavaScript (`naro-js`)

- `Element.attachShadow({mode})` (erro se já existir root), `shadowRoot` (devolve `null`
  para `closed`), `host`, `mode`, `getRootNode()`, `nodeType === 11` para shadow root.
- `HTMLElement` e `customElements.{define, get, upgrade, whenDefined}`.
- Upgrade disparado em `define` (para os elementos já no documento), `createElement`,
  `appendChild`, `insertBefore`, `append`/`prepend`, `innerHTML`, `insertAdjacentHTML`.
- `connectedCallback` só roda quando o nó está **de fato** no documento — um elemento
  criado por `createElement` tem o documento como pai provisório, então a checagem usa
  `__naro_in_document` (caminhada até a raiz), não `parent >= 0`.
- `disconnectedCallback` em `remove()` e `removeChild()`, na subárvore.
- Nome inválido (sem hífen) lança; nome repetido lança.
- Upgrade atravessa shadow trees: um `<x-item>` dentro de `root.innerHTML` é atualizado
  quando o construtor é definido depois.

### 2.5 Integração e diagnóstico (`naro-engine`)

- `compute_page_styles` centraliza: folha do documento + uma folha por shadow root,
  `compute_styles_scoped`, absolutização de `background-image`, e o `ParseReport`
  somado (as regras das árvores aparecem em `CSS: rules=N`).
- `--dump` imprime `#shadow-root (open|closed)` sob o host, com o conteúdo indentado.

## 3. Testes adicionados

| Crate | Testes | Cobre |
|---|---|---|
| `naro-html` (+4) | `attach_shadow_keeps_the_light_tree_separate`, `shadow_scope_names_the_enclosing_tree`, `collect_style_text_keeps_shadow_styles_out_of_the_document`, `elements_by_tag_sees_shadow_content` | modelo, escopo, isolamento de folha |
| `naro-css` (+8) | `host_pseudo_matches_the_host_only_from_its_tree`, `host_pseudo_takes_a_compound_argument`, `shadow_rules_style_only_their_own_tree`, `document_rules_do_not_reach_shadow_content`, `inherited_properties_flow_from_the_host_into_the_tree`, `shadow_sheet_variables_apply_inside_the_tree`, `nested_shadow_trees_do_not_leak_into_each_other`, `host_rules_lose_to_document_rules_and_win_when_important` | `:host`, encapsulamento nos dois sentidos, herança, variáveis, aninhamento, precedência |
| `naro-layout` (+1) | `shadow_content_is_laid_out_instead_of_the_light_children` | composição: só o conteúdo da árvore vira caixa |
| `naro-js` (+8) | `attach_shadow_exposes_the_root_and_its_content`, `closed_shadow_root_is_not_reachable_from_the_host`, `custom_element_connected_callback_runs_on_define`, `connected_callback_waits_until_the_element_is_on_the_page`, `custom_element_created_later_is_upgraded`, `custom_element_disconnected_callback_runs_on_remove`, `custom_element_inside_a_shadow_tree_is_upgraded`, `invalid_custom_element_names_throw` | API, `closed`, ciclo de vida, upgrade tardio, aninhamento, erro de nome |
| compat (+1 página) | `tests/compat/28-shadow-dom.html` | ponta a ponta: componente com `:host`, cor interna vs. externa, árvore criada por `setTimeout` |

## 4. Resultados

```
cargo test --workspace     219 passed, 0 failed        (antes: 198)
naro --compat              27 pass, 0 falhas, 1 unsup  (antes: 26/0/1 de 27)
```

Página 28, verificado por `--dump` e por captura (`reports/s28-shadow.png`):

| Elemento | Esperado | Obtido |
|---|---|---|
| host `x-card` | fundo `#681da8` (do `:host`) | `bg=#681da8` |
| `p.inside` interno | `#0a7d3f` (folha da árvore) | `color=#0a7d3f` |
| `p.inside` da página | `#8a1f1f` (folha do documento) | `color=#8a1f1f` |
| `p` criado por timer, em árvore nova | `#b45309` | `color=#b45309` |
| altura do host | = altura do conteúdo composto | 21,6 = 21,6 |

Páginas reais, sem regressão:

| Página | STATUS | CONTENT_HEIGHT | JS ERROR |
|---|---|---|---|
| `example.com` | Idle | 820,60 (inalterado) | 0 |
| `rust-lang.org` | Idle | 3968,98 (inalterado) | 0 |
| `google.com` | Idle | 487,90 (inalterado) | 8 pré-existentes, 0 novos |
| MDN (docs/Web/HTML) | Idle | 16897,00 (baseline 11322,00 — ver 5) | 6, idênticos ao baseline |

## 5. Limitações (não é Web Components completo)

- **Sem `<slot>`.** O conteúdo composto é o da shadow tree; os filhos light **não** são
  distribuídos nem renderizados. `<slot>`, `slot="..."`, `slotchange` e `::slotted()` não
  existem. Esta é a maior lacuna: componentes que dependem de conteúdo projetado aparecem
  vazios.
- **Sem `observedAttributes`/`attributeChangedCallback`/`adoptedCallback`.** As reações
  rodam de forma síncrona no ponto da mutação; não há fila de reações nem
  `customElements.upgrade` assíncrono.
- **Sem retargeting de eventos.** `dispatchEvent` bolha pela árvore (o shadow root tem o
  host como pai), mas `event.target` não é reescrito para o host ao cruzar a fronteira.
- **Sem `adoptedStyleSheets`/`CSSStyleSheet`.** Só `<style>` dentro da árvore.
- **Sem `:host-context()`, `::part()`, `exportparts`, `delegatesFocus`, `ElementInternals`.**
- **`attachShadow` não valida o elemento hospedeiro** (a spec restringe a elementos com
  nome válido de custom element e a um conjunto fixo de tags nativas).
- **`innerHTML` não clona shadow roots** — correto segundo a spec, mas significa que
  serializar e reparsear um componente perde a árvore.
- **`document.querySelector` não cruza fronteiras** (correto); não existe ainda
  `element.shadowRoot.querySelectorAll` com semântica de slot.
- **MDN: `CONTENT_HEIGHT` 16897 vs. 11322 do baseline.** A diferença é anterior a esta
  fase (o baseline foi capturado antes das correções de layout da sessão anterior, que
  removeram os ícones de 678–960 px). Não há `#shadow-root` na página — o MDN não usa
  shadow DOM nesta rota — e os 6 `JS ERROR` são exatamente os do baseline. Não foi
  investigado além disso porque está fora do escopo da Fase 6.

## 6. Arquivos alterados

| Arquivo | Alteração |
|---|---|
| `crates/naro-html/src/lib.rs` | `ShadowMode`, `NodeData::ShadowRoot`, `Document::shadow_roots`, 7 métodos novos, `collect_style_text_from`; +4 testes |
| `crates/naro-css/src/lib.rs` | `Pseudo::Host`, parse de `:host(...)`, `matches_scoped`, `has_host_pseudo`, `compute_styles_scoped` com escopo e precedência de encapsulamento; +8 testes |
| `crates/naro-layout/src/lib.rs` | `composed_children` usado nos dois walks de construção de fluxo; +1 teste |
| `crates/naro-js/src/lib.rs` | `attachShadow`/`shadowRoot`/`host`/`mode`/`getRootNode`/`nodeType 11`; `HTMLElement`, `customElements`, upgrade e ciclo de vida; 5 host functions novas; +8 testes |
| `crates/naro-engine/src/lib.rs` | `compute_page_styles` (folha por árvore + relatório somado) em 4 pontos; `--dump` mostra shadow roots |
| `tests/compat/28-shadow-dom.html` | página nova |
| `tests/compat/manifest.txt` | linha de 28 |
| `reports/s28-shadow.{bmp,png}` | captura da página 28 |
| `reports/sprint28.md` | este relatório |

Nenhuma alteração fora desses arquivos. `reports/layout-stabilization.md`, os relatórios
anteriores e os artefatos de diagnóstico foram preservados.

## 7. Nota de ambiente

O build com `+stable-x86_64-pc-windows-gnu` passou a falhar com
`error calling dlltool 'dlltool.exe': program not found` porque o PATH da sessão perdeu o
diretório do WinLibs. A correção é de ambiente, não de código: prefixar o PATH com
`.../WinGet/Packages/BrechtSanders.WinLibs.POSIX.UCRT_*/mingw64/bin` (separador `;`).
