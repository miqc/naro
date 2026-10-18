# Sprint 15 — CSS Grid responsivo

## Implementado

| Recurso | Onde | Observação |
|---|---|---|
| `minmax(min, max)` | `naro-css::parse_track` | `Track { px, fr, min_px, max_px }`; `min` vira piso, `max` vira teto |
| `repeat(auto-fit, …)` / `repeat(auto-fill, …)` | `parse_tracks` + `naro-layout::resolve_grid_columns` | contagem calculada como `floor((largura + gap) / (mínimo + gap))`, limitada a 12 |
| `grid-column` / `grid-row` | `parse_placement` | `2`, `span 2`, `1 / 3`, `1 / span 2` |
| spans no algoritmo | `Flow::grid` | ocupação de células, busca de espaço livre, itens com span de linha posicionados primeiro |
| `grid-area` / `grid-template-areas` | — | **não implementado** (exigiria nomear áreas no parser) |

`repeat()` com contagem fixa e `grid-template-columns: 1fr 1fr` continuam funcionando.

## Testes automatizados

`cargo test --workspace`: **54 passando, 0 falhas** (4 novos):

- `minmax_and_auto_fit_produce_adaptive_columns` — 410 px, `minmax(120px,1fr)`, gap 10 → 3 colunas de 130 px, mesma linha.
- `auto_fit_shrinks_the_column_count` — `minmax(200px,1fr)` → 2 colunas; o terceiro item quebra de linha.
- `grid_column_span_covers_two_columns` — `grid-column: span 2` cobre 410 px; o item seguinte desce.
- `grid_row_span_increases_the_row_height` — `grid-row: span 2` com `height:120px`; o container cresce para conter o item.

`naro --compat tests/compat`: **14 PASS, 0 FAIL, 1 UNSUP (total 15)**.
Baseline anterior era 13/0/1; o caso novo `15-grid.html` foi adicionado e passa.

## Validação visual

Página `tests/compat/15-grid.html`: grid com `repeat(auto-fit, minmax(160px,1fr))`,
gap 12, um item com `grid-column: span 2` (verde) e outro com `grid-row: span 2` (vermelho).

Render conferido (`g.png` no histórico da sessão):

- 4 colunas de ~185 px em 1000 px de largura.
- O item verde ocupa exatamente duas colunas (383 px) e o azul seguinte fica na quarta coluna.
- O item vermelho ocupa duas linhas e empurra os itens seguintes para a linha de baixo.
- Textos de comprimentos diferentes quebram dentro das células sem sobreposição.

## Páginas reais

| Página | Antes | Depois |
|---|---|---|
| example.com | ok | ok |
| rust-lang.org | ok | ok |
| MDN | lista vertical, sobreposição | **ainda lista vertical** |

`CONTENT_HEIGHT` do MDN caiu de 17190 para 11384 (o grid agora posiciona
itens que antes empilhavam), mas o layout de três colunas **não** apareceu.

### Causa concreta do MDN (não corrigida)

A regra que cria as três colunas é:

```css
:host { align-items:center; display:grid;
        grid-template-areas:"icon input button";
        grid-template-columns:2.2rem 1fr min-content }
```

Ela vive dentro de um **Shadow DOM** (`:host`). NARO não implementa Shadow DOM
nem `grid-template-areas`/`grid-area`. Sem isso, o seletor `:host` é descartado
e a barra de busca permanece em bloco.

Limitações adicionais observadas na mesma página: `[hidden]{display:none}`
(seletor de atributo), `mask-image`, `grid-template-areas`, `calc()`.

Não foram adicionadas regras específicas para o domínio.

## Limitações restantes

- Sem `grid-template-areas` / `grid-area` / `grid-column-start` nomeado por linha.
- `auto-fit`/`auto-fill` só com track de tamanho calculável; `minmax(auto, …)` cai para 1 coluna.
- `min-content`/`max-content` são tratados como `auto`.
- Sem `align-items`/`justify-items`/`align-content` no grid (só no flex).
- Sem Shadow DOM, o que bloqueia a maior parte dos componentes web modernos.

## Disco

12 GB livres antes e depois. Um build de release; nenhum diretório removido.
