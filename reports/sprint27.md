# Sprint 27 — Etapa 5: completude e robustez do layout

Escopo: Etapa 5 do roadmap. Etapas 1–4 preservadas; Shadow DOM, custom elements e
otimizações de memória não foram tocados.

## 1. Problemas reproduzidos

Cada defeito abaixo foi reproduzido com uma página local antes da correção, e virou
teste em `crates/naro-layout`.

| # | Sintoma | Causa | Onde |
|---|---|---|---|
| 1 | `flex-basis` era ignorado; `flex: 1` não existia | só `flex-grow` era lido, e o crescimento acontecia **duas vezes** (`flex_row` e `flex_line`) | `naro-css`, `naro-layout` |
| 2 | Item flexível encolhia até 0px e o texto virava uma letra por linha | faltava a "minimum content size" de `min-width: auto` | `naro-layout` |
| 3 | `margin-left: auto` não empurrava o item | só o atalho `margin` marcava margem automática | `naro-css` |
| 4 | `align-items: stretch` não esticava; `center` arrastava o irmão junto | não havia stretch, e o deslocamento usava `shift_subtree` (que move vizinhos) | `naro-layout` |
| 5 | `align-content` inexistente em flex e em grid | propriedade não parseada | `naro-css`, `naro-layout` |
| 6 | `minmax(300px, 1fr)` dava 300 + 1fr **somados** | `fr` era adicionado ao mínimo em vez de limitado por ele | `naro-layout` |
| 7 | `grid-template-columns: var(--x)` criava uma coluna invisível de 0px por token não resolvido | tokens inválidos viravam `Track{0}` em vez de serem descartados | `naro-css` |
| 8 | `min-content`/`max-content`/`fit-content` viravam track de 0px | não eram reconhecidos como "auto" | `naro-css` |
| 9 | Tabelas: colunas diferentes por linha, sem `colspan`/`rowspan`, sem `table-layout` | cada linha calculava as próprias larguras | `naro-layout` |
| 10 | `height: 20px` produzia 21,6px (o conteúdo vencia o declarado) | altura declarada era `max(conteúdo, declarada)` | `naro-layout` |
| 11 | `display: inline-block` não existia; `display: contents` também não | `Display` sem as variantes | `naro-css`, `naro-layout` |
| 12 | `position: fixed` era tratado como `absolute` | `"absolute" \| "fixed"` no mesmo braço | `naro-css`, `naro-layout` |
| 13 | `inline-block` com margem perdia a margem e quebrava o texto em duas linhas | a largura shrink-to-fit não somava as margens | `naro-layout` |
| 14 | `max_content_width` de um bloco com palavras colava `+ w` dentro do `if` | precedência: `a + if x {b} else {0}` parseia como `a + (b if …)` | `naro-layout` |

## 2. Implementações

### 2.1 Dimensionamento e fluxo

- `height` declarado é **definitivo**: a caixa mantém a altura e o conteúdo
  transborda (recortado só se `overflow` pedir). Antes a altura crescia até o
  conteúdo, o que impedia qualquer layout de altura fixa.
- `min-height`/`max-height` (novos) aplicados depois do cálculo, via
  `clamp_height`, em block, flex e tabelas.
- `display: inline-block`: caixa inline que faz shrink-to-fit, contém blocos e
  reserva a **altura real** na linha (`self.block(...)` antes de `place_atom`).
- `display: contents`: sem caixa; os filhos entram no fluxo do avô, herdando o
  contexto do contêiner (`build_children_into(..., container)`).
- `position: fixed`: caixa fora do fluxo, resolvida contra a **viewport**
  (`layout_with_viewport` leva altura e largura reais do engine).
- `is_out_of_flow` cobre `absolute` e `fixed` em todos os pontos que filtram
  itens (block, flex, grid, absolutos coletados).

### 2.2 Flexbox

Reescrito em torno de `flex_row` + `flex_line`:

1. **Base**: `flex-basis` → `width` → `max-content`, mais margem/padding/borda.
2. **Linhas**: com `flex-wrap`, a quebra acontece **antes** de crescer.
3. **Grow/shrink** por linha: o crescimento divide o espaço livre por `flex-grow`;
   o encolhimento divide o déficit por `flex-shrink × base` (não por base).
4. **Clamp** por item: `max-width`, `min-width` e, sem `min-width` explícito, o
   mínimo de conteúdo (`min_content_width`) — o que impede um item de virar coluna
   de uma letra.
5. **Distribuição**: margens automáticas absorvem o espaço livre antes de
   `justify-content`; `start/center/end/space-between/space-around` como antes.
6. **Alinhamento vertical**: `align-self`/`align-items` com `center`/`end` movendo
   só a subárvore do item, e `stretch` esticando a caixa até a linha.
7. **`align-content`** distribui as linhas quando o contêiner tem altura definida.

`block()` aceita a largura decidida pelo flex via `Flow::flex_width`: um item pode
ter sido crescido ou encolhido, e reaplicar `width`/`min`/`max` desfaria a conta.

### 2.3 Grid

- `resolve_tracks`: trilhas fixas (`px`, `%`) mantêm o tamanho; `fr` divide o que
  sobra **depois** delas; o resultado é limitado por `minmax()` (`min` como piso,
  `max` como teto) em vez de somado.
- Tracks não parseáveis são **descartados** (`parse_track` devolve `Option`), então
  `var(--ausente)` não gera colunas de 0px.
- `min-content`/`max-content`/`fit-content` são tratados como `auto`.
- Alinhamento vertical dos itens na linha (`align-items`/`align-self`, com
  `stretch`), aplicado depois que todas as alturas de linha são conhecidas.

### 2.4 Tabelas

- `table_columns` decide **uma** grade de colunas para a tabela inteira: largura
  máxima de conteúdo por coluna (respeitando `colspan`), depois ajuste ao width da
  tabela.
- `table-layout: fixed`: só a primeira linha define as larguras; colunas sem
  largura declarada dividem o restante igualmente.
- `colspan`/`rowspan` lidos dos atributos em `build_box`; `rowspan` marca as
  colunas ocupadas para as linhas seguintes e a célula é esticada até a última
  linha que cobre.
- Tabela com `width` declarado distribui a sobra entre as colunas.

### 2.5 Diagnóstico

`--style <url> <seletor>` imprime o estilo computado (display, posição,
width/height/min/max, flex, grid, margens) de cada nó que casa. Foi o que permitiu
isolar os defeitos 6–8 sem adivinhar.

## 3. Arquivos alterados

| Arquivo | Responsabilidade |
|---|---|
| `crates/naro-css/src/lib.rs` | `Display::InlineBlock`/`Contents`; `Position::Fixed`; `flex-shrink`, `flex-basis`, `flex` (atalho e keywords), `align-content`, `min-height`, `max-height`, `table-layout`; `AlignContent`; `parse_track` → `Option<Track>`; `min-content`/`max-content` como `auto`; `margin-*: auto` por lado |
| `crates/naro-layout/src/lib.rs` | `is_block_level`, `is_out_of_flow`, `build_children_into`, `min_content_width`, `clamp_height`, `flex_row` novo, `flex_line` com auto-margens/stretch, `table_columns`, `table_row` com spans, `resolve_tracks` corrigido, alinhamento de grid, altura declarada definitiva, `layout_with_viewport`, `Flow::flex_width`, correção de precedência em `max_content_width` |
| `crates/naro-engine/src/lib.rs` | `layout_with_viewport` com altura real da viewport (carga, relayout, layout provisório, medição para JS) |
| `crates/naro-app/src/main.rs` | `--style <url> <seletor>` |
| `README.md` | Seção de layout reescrita, `--style` documentado |

## 4. Testes

`cargo test --workspace` → **169 testes, 0 falhas** (146 → +23).

Novos em `naro-layout` (21): `flex_basis_and_shorthand_control_the_base_size`,
`flex_shrink_is_weighted_by_the_base_size`, `flex_shrink_zero_keeps_the_item_size`,
`flex_min_and_max_width_bound_the_result`, `flex_auto_margins_push_items_apart`,
`align_content_distributes_wrapped_lines`, `align_items_stretch_fills_the_line`,
`align_items_center_offsets_the_item`, `table_columns_are_shared_between_rows`,
`table_colspan_spans_the_columns`, `table_rowspan_extends_the_cell`,
`table_layout_fixed_takes_widths_from_the_first_row`, `display_contents_removes_the_box`,
`inline_block_shrink_wraps_and_keeps_its_height`, `min_and_max_height_bound_the_box`,
`fixed_position_uses_the_viewport`, `minmax_floors_a_flexible_track`,
`fr_tracks_share_the_space_left_by_fixed_tracks`,
`unresolvable_grid_tracks_do_not_create_columns`,
`grid_align_items_centers_the_item_in_its_row`, `grid_stretch_fills_the_row_height`.

Novos em `naro-engine` (2): `a_width_change_relayouts_and_repaints`,
`measurements_track_flex_layout`.

Compatibilidade: `naro --compat tests/compat` → **25 pass, 0 falhas, 1 unsup (26)**,
igual à Etapa 4. Nenhuma página de compat precisou de ajuste.

## 5. Benchmarks

`--bench` em `naro://start`, três execuções:

| | Etapa 4 | Etapa 5 |
|---|---|---|
| parse | 0,018 ms | 0,018 ms |
| style | 0,39–0,47 ms | 0,43–0,45 ms |
| layout | 0,080–0,125 ms | 0,084–0,088 ms |
| paint | 0,53 ms | 0,53 ms |

Mesma faixa. O layout ficou mais estável (0,084–0,088 contra 0,080–0,125) porque
tabelas e flex agora fazem uma passada de medição em vez de recalcular por linha.

## 6. Páginas de referência

| Página | Etapa 4 | Etapa 5 | JS ERROR | Textos |
|---|---|---|---|---|
| `naro://start` | 606,40 | 606,40 | 0 = 0 | idênticos |
| `example.com` | 776,60 | 776,60 | 0 = 0 | idênticos |
| `rust-lang.org` | 4338,38 | **3940,18** | 0 = 0 | nenhum perdido |
| `google.com` | 480,33 | **482,48** | 0 = 0 | nenhum perdido |
| MDN `/Web/HTML` | 12039,00 | **16378,00** | 6 = 6 | nenhum perdido |

Nenhum texto perdido em nenhuma das cinco páginas (checagem por substring em todos
os runs), nenhum `JS ERROR` novo, nenhum `STATUS: Error`.

- **rust-lang** encolheu 9%: a barra de navegação era uma coluna estreita porque
  `flex` não distribuía espaço; agora os itens do `nav` ocupam a linha, e o
  cabeçalho deixa de empilhar duas linhas por item.
- **google.com** subiu 2px: os links do rodapé (`display: inline-block`) passaram a
  caber em uma linha (`Publicidade`, `Soluções empresariais`, `Sobre o Google`,
  `Google.com.br` na mesma altura), como no navegador.
- **MDN** cresceu 36%. O grid da página usa `grid-template-columns:
  var(--layout-2-sidebars)`, e essa variável está nos **arquivos CSS externos**, que
  o NARO não carrega (limitação registrada desde o início). Sem ela o grid fica com
  as trilhas que sobraram e a coluna de conteúdo fica estreita; o texto continua
  todo presente, só que em colunas mais altas. Não é regressão introduzida aqui:
  é a mesma causa raiz de antes (CSS externo ausente), agora visível porque o grid
  finalmente posiciona itens em vez de empilhá-los.

Capturas: `reports/p5-google.png`, `reports/p5-rust.png`, `reports/p5-mdn.png`;
dumps em `reports/p5-*.txt`.

## 7. Regressões

Nenhuma regressão atribuível a esta etapa. O único delta de conteúdo é o token de
sessão que o Google sorteia por requisição (`kEI`, `sig`), presente em qualquer par
de dumps consecutivos.

## 8. Limitações restantes

- `border-collapse` não implementado: cada célula desenha a própria borda.
- `vertical-align` em células e em inline continua sem efeito (a linha alinha pela
  linha de base do texto).
- `order`, `flex-flow`, `grid-area`, linhas/colunas nomeadas e `grid-auto-flow`
  permanecem fora.
- `justify-items`/`justify-self` no grid não existem; só o eixo de bloco é alinhado.
- `min-height`/`max-height` não aceitam percentual (precisariam da altura do
  contêiner, que o fluxo de blocos não fixa).
- `overflow: auto`/`scroll` recortam como `hidden`, mas não criam barras de rolagem.
- `position: sticky` não implementado.
- `float` não implementado.
- Grid com `fr` e `%` na mesma trilha soma as duas contribuições.
- Layout provisório (Etapa 4) roda uma vez a mais em páginas com `<script>`.
- MDN continua dependendo de CSS externo; nenhuma correção por domínio foi feita.

## 9. Etapas 1–4

- Etapa 1 (seletores/cascata): nenhuma alteração.
- Etapa 2 (unidades/funções): `Dim`/`LengthCtx` intactos; `min-height`/`max-height`
  reutilizam `parse_dim`.
- Etapa 3 (invalidação): `Invalidation`, `flush` e `MutationObserver` intactos; as
  novas propriedades entraram em `geometry_differs` (`flex_shrink`, `flex_basis`,
  `align_content`, `min_height`, `max_height`, `table_fixed`), então uma mudança de
  `flex-basis` corretamente provoca relayout.
- Etapa 4 (DOM/medição): `ElementBox` e `set_geometry` intactos; a geometria passou a
  refletir o layout novo, o que os dois testes de engine verificam.
