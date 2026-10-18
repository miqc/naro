# Sprint 25 — Fase 3: CSS dinâmico e invalidação

Escopo: Fase 3 do plano mestre. Fases 1 e 2 preservadas; Fase 4 não iniciada.

## 1. Resumo

O ciclo de atualização passou a existir. Antes, `sync_from_js` fazia recascata +
relayout **inteiros** a cada clique, tecla ou timer, sem olhar o que mudou, e
`<style>` era um `String` congelado no parse — qualquer folha criada, alterada ou
removida por JavaScript simplesmente não existia para a cascata.

Agora:

- o texto das folhas é lido do DOM (`Document::collect_style_text`), então inserir,
  alterar ou remover `<style>` chega à cascata sem bookkeeping extra;
- cada mutação marca `Invalidation { style, layout, paint }` no runtime JS;
- a engine consolida tudo uma vez por tarefa em `Engine::flush`, na ordem
  estilo → layout → pintura;
- o relayout só acontece quando a geometria pode mudar, decidido por um diff de
  `ComputedStyle` (`geometry_differs`/`geometry_changed`);
- quando só a pintura muda, `Layout::refresh_paint` copia cores/fundos/bordas para as
  caixas e runs existentes, sem recalcular nenhuma posição.

## 2. Arquivos modificados

| Arquivo | Responsabilidade |
|---|---|
| `crates/naro-html/src/lib.rs` | `style_text` congelado removido; `Document::collect_style_text()` varre `<style>` em ordem de documento a cada recascata |
| `crates/naro-css/src/lib.rs` | `ComputedStyle::geometry_differs` e `geometry_changed` (quais propriedades movem caixas); `PartialEq` em `Track`/`AutoRepeat`/`GridPlacement` |
| `crates/naro-js/src/lib.rs` | `Invalidation` + contadores no `Dom`; host `__naro_invalidate`; `__naro_mutation` invalida **sempre** (mesmo sem observer); `id`/`innerHTML`/`innerText`/`appendChild`/`insertBefore`/`removeChild` passam a emitir registro; `value` marca só pintura; `invalidation()`/`take_invalidation()` |
| `crates/naro-layout/src/lib.rs` | `Layout::refresh_paint` (cores de blocos/runs e fundo da página); `TextRun.node` e `background_node` para localizar o estilo de origem |
| `crates/naro-engine/src/lib.rs` | `dirty`, `restyle_count`, `relayout_count`, `repaint_count`; `flush()` substitui `sync_from_js`; `pump_js` drena mutações pendentes sem timer; `SYNC:` no `--dump` |
| `crates/naro-app/src/main.rs` | Checks `click:SEL`, `restyle:N`, `relayout:N`, `noruncolor:RRGGBB`, `blockbg:tag:RRGGBB`; `--render` aceita coordenadas de clique |
| `tests/compat/25-dynamic-style.html`, `manifest.txt` | Página da fase e checks |
| `README.md` | Seção "Atualização dinâmica", APIs de JS realmente implementadas, `--render` com clique |

## 3. Arquitetura do ciclo

```
mutação JS ──► __naro_mutation(kind, ...)
                 │
                 ├─► __naro_invalidate("style")  → Invalidation{style, paint}
                 └─► fila do MutationObserver (microtask) → callbacks
                        │
tarefa termina (fim do handler / timer / microtasks)
                 │
                 ▼
        Engine::flush()  ── take(invalidation)
                 │
        style?  ── recascata (collect_style_text → parse → compute_styles)
                 │   ├─ geometry_changed? → promove para layout
                 │   └─ senão → Layout::refresh_paint (contador repaint)
        layout? ── relayout completo (layout_document)
        paint   ── nada a fazer aqui: o chamador redesenha o frame
```

`flush()` é idempotente: sem nada pendente retorna `false` e não incrementa contador
nenhum. É isso que impede um laço — a recascata não gera nova invalidação.

Pontos de entrada que chamam `flush` uma vez por tarefa: `click_at`, `emit`
(eventos de foco/valor/tecla), `pump_js` (frame loop). `set_control_value` (digitação)
marca apenas pintura.

## 4. Regras de invalidação

| Alteração | style | layout | paint | Onde é decidido |
|---|---|---|---|---|
| `classList`, `className`, `id`, `setAttribute`, `removeAttribute`, `style.*` | sim | se o diff de estilo mudar geometria | sim | `__naro_mutation` + `geometry_changed` |
| `<style>` inserido, alterado, removido (via `childList`/`textContent`) | sim | idem | sim | `__naro_mutation("childList")` |
| `appendChild`, `insertBefore`, `removeChild`, `innerHTML`, `innerText`, `textContent` | sim | **sim** (contagem de nós muda) | sim | `geometry_changed` (tamanhos diferentes ⇒ `true`) |
| `input.value` / digitação | não | não | sim | `__naro_invalidate("paint")` |
| Resize da janela | — | sim (só largura) | sim | `set_viewport` (inalterado) |

Propriedades que contam como geometria (`geometry_differs`): `display`, `font_size`,
`font_weight`, `margin`, `padding`, `border`, `text_align`, `line_height`, flex/grid
completo, `width`/`height`/`min`/`max`, `position` e offsets, `overflow`, `list_marker`,
`border_box`, `gap`. Fora do diff (pintura pura): `color`, `background`,
`border_color`, `border_radius`, `box_shadow`, `underline`, `background_image`.

Fallback conservador: qualquer diferença na contagem de nós força layout, e qualquer
propriedade não listada como pintura pura entra na comparação — o padrão é
"recalcular", nunca "assumir que não mudou".

## 5. Integração com DOM, CSS e MutationObserver

- **DOM**: a fonte da verdade continua sendo o `Document` do `JsSession`. `flush` toma
  um `snapshot()` uma vez por tarefa.
- **CSS**: mesmo `parse_stylesheet_for` + `compute_styles_with` da Fase 2, agora
  alimentados por `collect_style_text()`. Nenhum parser paralelo.
- **MutationObserver**: a fila de registros e a entrega em microtask não mudaram.
  O que mudou é que a invalidação deixou de depender do observer (`if (observers.length
  === 0) return;` acontecia **antes** de qualquer invalidação) e que `appendChild`,
  `insertBefore`, `removeChild`, `innerHTML`, `innerText` e o setter de `id` passaram a
  produzir registros — antes só `textContent`, `classList`, `setAttribute` e `style`
  notificavam. Mutações feitas dentro de um callback de observer continuam sendo
  entregues na rodada seguinte (microtask), sem reentrância.

## 6. Testes e resultados

`cargo test --workspace` → **133 testes, 0 falhas** (110 → +23).

Novos em `naro-engine` (14, todos com viewport fixo e DOM real via JS):

| Teste | Cobre |
|---|---|
| `class_change_restyles_without_relayout` | add de classe muda a cor; `relayout=0`, `repaint=1` |
| `class_removal_restores_the_previous_style` | remove de classe volta à cor base |
| `attribute_change_reaches_the_cascade` | `setAttribute('data-state','on')` |
| `inline_style_change_applies` | `el.style.color = ...` |
| `inserted_style_element_applies` | `<style>` criado por `createElement` + `appendChild` |
| `rewritten_style_element_applies` | `textContent` de um `<style>` existente |
| `removed_style_element_reverts_the_cascade` | `removeChild` da folha; texto da folha encolhe |
| `inserting_and_removing_nodes_relayouts` | insere `<p>` e remove outro; `relayout=1`, textos atualizados |
| `mutations_in_one_task_group_into_one_restyle` | 4 mutações em um handler → `restyle=1` |
| `mutation_observer_delivers_the_changes` | `attributes` + `attributeName` no callback |
| `observer_callback_mutations_do_not_loop` | cadeia de 3 rodadas e para; `restyle ≤ 2` |
| `typing_only_repaints` | digitar não recascata nem relayouta |
| `camel_case_inline_style_is_applied` | `style.backgroundColor` grava `background-color` e chega à cascata |
| `flush_without_work_is_a_noop` / `pump_js_flushes_pending_mutations_without_timers` / `a_flushed_mutation_does_not_retrigger_itself` | idempotência e ausência de laço |

Novos em `naro-css` (2): `geometry_differs_only_for_layout_properties`,
`geometry_changed_detects_a_different_node_count`.
Novos em `naro-layout` (1): `paint_refresh_updates_colours_but_not_geometry`
(geometria idêntica, cor de run e fundo da página atualizados).
Novos em `naro-js` (4, primeiro teste do crate): `paint_only_never_implies_layout`,
`style_implies_paint_but_not_layout`, `layout_implies_paint`, `merge_unions_every_flag`.

Compatibilidade: `naro --compat tests/compat` → **24 pass, 0 falhas, 1 unsup (25)**
(era 23/0/1). `25-dynamic-style.html` aplica, num único clique:

```
#target  add+remove de classe → cor base     #target2 classe → #0a7d3f
#box     data-state=on        → #681da8     #inline  style → #8a1f1f
<style>  inserido            → #b45309      <style>  reescrito → #0f766e
<style>  removido            → #0e7490 some  nós      +1/-1 (elemento novo)
observer attributes:data-state                restyle=1  relayout=1
segundo clique (só cor)      → restyle=2  relayout=1  (sem relayout novo)
#box     style.backgroundColor (camelCase) → fundo #0e7490, sem relayout
```

Verificação visual: `reports/s25-dynamic.png` (estado inicial) e
`reports/s25-after-click.png` (após o clique) mostram "estilo removido" passando de
teal para preto, "novo estilo" laranja, "estilo trocado" teal e "elemento novo" no
lugar de "elemento removido".

## 7. Baseline S24 → S25

| Página | CONTENT_HEIGHT | JS ERROR | Dump |
|---|---|---|---|
| `naro://start` | 606,40 → 606,40 | — | idêntico |
| `example.com` | 315,80 → 315,80 | `s.js` pré-existente | idêntico |
| `rust-lang.org` | 4338,38 → 4338,38 | — | idêntico |
| `google.com` | 480,33 → 480,33 | — | idêntico exceto o token de sessão que o Google sorteia por requisição |
| MDN | 12039,00 → 12039,00 | 6 = 6 | idêntico |
| `tests/compat/24-units.html` | inalterada (PASS) | — | inalterado |

`SYNC: restyle=0 relayout=0 repaint=0` em todas: carregar uma página não gera
recascata extra. `--bench` (`naro://start`): style 0,24–0,28 ms, layout 0,07–0,09 ms —
mesma faixa da Sprint 24.

Nenhuma regressão nova. O único delta de texto é o `kEI`/`jsr` que o Google gera por
carregamento, presente em qualquer par de dumps consecutivos.

## 8. Incremental × fallback

| Etapa | Situação |
|---|---|
| Detecção do que mudou | **incremental** (flags por mutação) |
| Recascata | **completa** para o documento (sem invalidação por subárvore) |
| Relayout | **completo**, mas **evitado** quando a geometria não muda |
| Atualização de pintura | **incremental por caixa** (`refresh_paint` copia só atributos visuais: cor de run, fundo/borda/raio/sombra de bloco e fundo da página) |
| Redesenho do frame | **completo** (o renderer não tem dano por região) |

Não há invalidação parcial de subárvore: o diff de estilo é O(nós) e a recascata é
total. Declarar seletor→nó como incremental exigiria índices que a Fase 7 pode
introduzir.

## 9. Limitações e não implementado

- Recascata completa por mutação de estilo; nenhuma reutilização de regras por seletor.
- `flush` compara todos os estilos para decidir layout: O(nós) por tarefa, não O(nós
  afetados).
- Sem `getBoundingClientRect`/`offsetWidth` (Fase 4): não há leitura de geometria
  síncrona, então nada força um `flush` antecipado.
- Mutações feitas *fora* de JS (ex.: `set_control_value` do host) marcam pintura
  manualmente; não existe API pública para invalidar de fora.
- `innerHTML` continua sem parsear o HTML atribuído (comportamento anterior, Fase 4).
- `style` converte camelCase para kebab-case (`backgroundColor` →
  `background-color`); `cssFloat` e prefixos de fornecedor continuam sem tradução.
- Repintura é sempre o frame inteiro.
- `MutationObserver` não tem `characterData` nem `takeRecords` fora do wrapper atual.
