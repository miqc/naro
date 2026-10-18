# Sprint 22 — Tabelas HTML e árvore de caixas CSS

Captura: `reports/google_s22.png` (comparar com `google_s21.png`).

## Causas confirmadas

| # | Evidência | Causa |
|---|---|---|
| 1 | `table`, `tr`, `td` existiam no DOM, mas não geravam caixas próprias | Não havia `Display::Table/TableRow/TableCell` nem regras UA; os nós eram achatados como inline, e o ancestral `position:relative` de dentro do `<td>` nunca virava bloco contentor |
| 2 | `@media` com lista por vírgula era avaliado como conjunção | `@media A, B` só casava se **as duas** condições fossem verdadeiras; listas OR eram rejeitadas |
| 3 | `min-width: 64rem` era ignorado silenciosamente | `media_matches` só entendia `px`; unidades `em`/`rem` falhavam no parse e a condição era descartada sem decisão |

## Implementação

| Frente | Arquivo | Detalhe |
|---|---|---|
| **Tabelas** | `naro-css` | `Display::Table/TableRow/TableCell`; UA `table{border-spacing:2px}`, `tr`, `td/th{padding:1px}` |
| | `naro-layout` | `table()` (linhas topo a baixo), `table_row()` (células lado a lado, largura intrínseca e encolhimento proporcional), `table_cell()` (bloco contentor, clip, posicionamento relativo/absoluto); `collect_rows()` atravessa `thead`/`tbody`/`tfoot`; fallback para layout de bloco quando `display:table` não tem linhas |
| **Media queries** | `naro-css` | Listas por vírgula avaliadas como OR; `parse_media_length` com `px`/`em`/`rem` (base 16); unidade desconhecida **não** casa |

## Testes automatizados

`cargo test --workspace`: **65 passando, 0 falhas** (3 novos em `naro-layout`):

- `table_places_cells_in_rows_and_columns` — 2×2, células da mesma linha com mesmo `y`, colunas alinhadas, `width:400px` respeitado.
- `table_cells_shrink_to_fit_the_table` — duas células de texto longo em tabela de 100 px somam ≤ 101 px.
- `relative_cell_contains_absolute_image` — `<td style="position:relative">` gera caixa com largura e altura.

`naro --compat tests/compat`: **21 PASS, 0 FAIL, 1 UNSUP (total 22)**, com `22-table-layout.html` novo (duas linhas, célula relativa com imagem absoluta, texto após a tabela).

## Validação visual

`tests/compat/22-table-layout.html` renderizado e conferido: tabela de 420 px, duas linhas, bordas visíveis, células alinhadas, ícone 12×12 dentro da célula relativa, texto "depois da tabela" abaixo.

**Google (`google_s22.png`) — mudança real:** o ícone do teclado saiu de x=968 e agora fica **dentro do campo, em x=481**, encostado na borda direita do input (que vai de 271 a 729). O logo, os botões ("Pesquisa Google" / "Estou com sorte") e o rodapé continuam centralizados. A tabela da barra de busca passou a produzir a hierarquia de caixas correta.

O campo continua sem `border-radius`/sombra: como registrado na sprint 21, esta variante da home **não serve essas regras** — não há o que aplicar.

## Regressão encontrada e não resolvida

`rust-lang.org` mudou de 4309,68 para **3233,16** px de altura, e o hero passou a
quebrar linha numa coluna estreita. Causa medida por A/B com
`NARO_NO_MEDIA=1` (4309,68) versus media queries ativas (3233,16): as regras
desktop agora entram na cascata, incluindo
`.max-width-quarter-l { max-width: 25% }`, que a engine aplica — mas o contêiner
pai que daria contexto a essa coluna usa grid com `minmax()`/`1fr` e não é
layoutado como no navegador. **A media query está correta; o layout de grid do
site é que não a acompanha.**

`example.com` inalterado em conteúdo; ganhou um erro JS novo
(`https://example.com/s.js: not a function`) porque scripts externos passaram a
ser carregados de fato — o script usa `insertAdjacentHTML`, ainda ausente.

## Comportamentos de tabela não suportados

- `rowspan` / `colspan`.
- `table-layout: fixed`, `border-collapse`, `caption-side`.
- Larguras de coluna compartilhadas entre linhas (cada linha calcula as suas).
- `col` / `colgroup` como definidores de largura.

## Erros JavaScript restantes

- Google: nenhum.
- example.com: `s.js: not a function` (`insertAdjacentHTML` ausente).
- MDN: `not a function @ <input>:5:11` (não investigado).

## Disco

12 GB livres antes e depois. Três builds de release; nenhum diretório removido.
