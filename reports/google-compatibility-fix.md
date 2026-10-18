# Correção de compatibilidade do Google.com

Alvo: a página real, carregada por HTTPS. Nenhuma regra, seletor ou coordenada
específica de domínio foi adicionada ao motor — cada correção abaixo é um defeito
genérico reproduzido com uma fixture local e coberto por teste.

## 1. Estado inicial reproduzido

`--render https://www.google.com` antes de qualquer mudança (`reports/g-before.png`):

- página praticamente vazia: só "Fazer login" e uma linha de texto;
- `CSS: rules=250` — apenas a folha inline do `<style>` no `<head>`;
- nenhum logo, nenhum campo de pesquisa visível, nenhum botão;
- `CONTENT_HEIGHT: 482,48`; **0 `JS ERROR`** (os scripts do Google nem chegavam a
  rodar porque quase não eram baixados).

Comparação com o Chrome (viewport 1000×700): logo centralizado, barra de pesquisa
branca com borda `#dadce0` e raio 26px, dois botões cinza, cabeçalho com Gmail/
Imagens/grade, rodapé em duas colunas.

## 2. Causas-raiz confirmadas

Cada uma foi confirmada com `curl` e/ou com `--style`, nunca por suposição.

| # | Sintoma | Causa | Confirmação |
|---|---|---|---|
| 1 | Nenhum CSS externo aplicado | O Google responde **200 com 0 bytes** de CSS para `User-Agent: NARO/0.1`. `curl -A "NARO/0.1"` → 0 bytes; `curl -A "Mozilla/5.0 … Chrome/141 … NARO/0.1"` → 3.096 bytes. Sem cookie, sem Referer, sem Accept: é só o UA. | `curl` nas duas variantes |
| 2 | `background: var(--xhUGwc)` ignorado; barra cinza | `parse_declarations` fazia `to_ascii_lowercase()` no nome de **toda** declaração, mas custom properties são case-sensitive: `--xhUGwc` virava `--xhugwc` e a referência nunca resolvia | fixture `bg.html`: `bg=#000000` antes, `bg=#ffffff` depois |
| 3 | Logo ausente | `<svg viewBox="0 0 272 92">` — o motor não desenhava SVG. Pior: o SVG era tratado como caixa de bloco vazia e herdava `width:100%`, virando um quadrado preto de 960px | `--style svg` → `width=Some(1000.0)` |
| 4 | Ícones do campo invisíveis | mesmo motivo do #3 (paths) | `--style svg` |
| 5 | Botões "Pesquisa Google"/"Estou com sorte" sobrepostos e fora de ordem | Cada átomo inline centralizava a si mesmo (`x + (largura - caixa)/2`), então dois botões centrados caíam no mesmo x | fixture `cen.html`: dois inputs de 150px ambos em x=425 |
| 6 | Logo à esquerda em vez de centralizado | `align-items: center` não existia no eixo cruzado de um flex column; `justify-items`/`place-items` não eram parseados | fixture `fc.html` + `--style .sI1XGe` |
| 7 | Campo de pesquisa com largura 0 | `max-content` de um flex row media só o item mais largo, não a soma; itens `flex: 1 1 0%` mediam 0 | fixture `mx.html`: linha de 8px |
| 8 | Overlay de ícone absoluto com 1000px de largura | caixa absoluta sem `width` usava a largura inteira do bloco contentor (CSS 2.1 §10.3.7 manda shrink-to-fit) | `--style .VHYc8b` |
| 9 | Elementos `visibility:hidden`/`opacity:0` pintados | propriedades não parseadas | `g-vis.png` |
| 10 | `.append of undefined` | `document.head` não existia | `--dump` → `JS ERROR` |
| 11 | Enter/botão não pesquisavam | não havia submit de formulário; `press_enter` só disparava `change` | teste `enter_submits_the_enclosing_form` |
| 12 | Imagens pintadas duas vezes | laço duplicado em `paint_page` | leitura do código |

## 3. Correções implementadas

### 3.1 Rede (`naro-net`)

`USER_AGENT` passa a ser um Chrome com o token do NARO no fim. É a única mudança
motivada pelo comportamento de um site, e é genérica: qualquer servidor que filtre
por UA deixa de servir 0 bytes. Registrada no README.

### 3.2 Cascata (`naro-css`)

- **Custom properties preservam a caixa** (`--xhUGwc` ≠ `--xhugwc`); nomes de
  propriedade comuns continuam em minúsculas.
- Novas propriedades: `opacity`, `visibility`, `justify-items`, `justify-self`,
  `place-items`.
- `visibility` é herdada; `opacity` multiplica a do pai (aproximação de composição).

### 3.3 Layout (`naro-layout`)

- **SVG inline** como elemento substituído: tamanho por atributos/`viewBox`, razão
  intrínseca quando não há tamanho, `fill` com `currentColor`/`none`/herança, paths
  achatados (`M/L/H/V/C/S/Q/T/Z`, arcos viram retas) e preenchidos pela regra
  *nonzero*. Geometria em `naro-layout/src/svg.rs` (pura, testada); `naro-paint`
  só plota.
- **Alinhamento de linha com átomos**: inputs, botões, imagens e SVG são colocados
  da esquerda para a direita e a linha inteira recebe o deslocamento de
  `text-align` no `flush`, movendo caixa, subárvore, runs e imagens do átomo.
- **Flex column**: `align-items`/`align-self` no eixo cruzado (largura + offset),
  que é como o Google centraliza o logo.
- **`max-content` de flex row**: soma dos itens, não o máximo; item com
  `flex-basis: 0` contribui com o próprio conteúdo.
- **Caixa absoluta sem `width`**: shrink-to-fit, não a largura do contentor.
- **`justify-items`** no grid: item mais estreito que a área é centrado/alinha ao
  fim.
- **Tolerância de quebra** (`FIT_EPSILON = 0.05px`): um shrink-to-fit que fecha
  exatamente no tamanho da palavra não quebra a última letra para a linha seguinte.
- **`visibility`/`opacity`** propagadas para caixas, runs e shapes (`paint`,
  `opacity`), com `refresh_paint` mantendo-as em dia.

### 3.4 Engine e JS

- `Engine::submit_from`: monta a query `GET` a partir dos controles com `name`
  (ignora `disabled` e `submit`/`button`/`image`/`file`), percent-encoding de
  formulário, e entrega a URL em `take_pending_nav()` — a engine não navega sozinha.
- `press_enter` submete o formulário do campo focado; clique em controle `submit`
  submete quando nenhum handler consumiu o clique.
- `document.head` implementado.
- `--style <url> <seletor>` ganhou atributos, cor, fundo, raio e `text-align`.
- `--render` aceita coordenadas de clique.

## 4. Comparação antes/depois

| | Antes (`g-before.png`) | Depois (`g-final4.png`) |
|---|---|---|
| Logo | ausente | centralizado, 272×92, cores corretas |
| Campo de pesquisa | 0px de largura | barra branca, borda `#dadce0`, raio 26px, ícones à direita |
| Botões | ausentes/sobrepostos | "Pesquisa Google" e "Estou com sorte" lado a lado, centralizados |
| Cabeçalho | só "Fazer login" | Gmail, Imagens, grade de apps, "Fazer login" azul |
| Rodapé | uma linha solta | duas colunas, seis links |
| `CSS: rules=` | 250 | **1.276** |
| `CONTENT_HEIGHT` | 482,48 | 493,60 |

## 5. Interação verificada

Testes em `crates/naro-engine` com a página real (`google_search_field_accepts_text`):

| Passo | Resultado |
|---|---|
| Focar o campo (`textarea.gLFyf`) | `focus == node` |
| Digitar "naro" | `control_value_of == "naro"` |
| Enter | `pending_nav == "https://www.google.com/search?…q=naro"` |
| Clique em submit | submete (teste genérico `clicking_a_submit_control_submits_the_form`) |
| Handler JS no submit | cancela a submissão (`a_js_handler_can_cancel_the_submission`) |
| Links (Gmail, Imagens, rodapé) | `hit_link` resolve e `navigate_to` navega — 9 links com `href` na página |
| SVG do campo (ícones) | desenhados, 20×20 |

## 6. Testes e regressões

`cargo test --workspace` → **193 testes, 0 falhas** (188 → +5: 2 em `naro-css`,
3 em `naro-layout`).

Novos testes desta rodada:

- `custom_properties_keep_their_case`
- `justify_items_centers_a_narrow_grid_item`
- `centered_line_aligns_its_atoms_together`
- `justify_items_centers_a_grid_item`
- `a_flex_row_of_percentage_width_svg_keeps_its_intrinsic_size`

Além dos já adicionados durante o trabalho: `inline_svg_is_laid_out_and_records_its_paths`,
`svg_without_size_uses_the_viewbox_ratio`, `svg_current_color_uses_the_element_color`,
`flex_column_centers_items_on_the_cross_axis`, `flex_row_max_content_adds_items_up`,
`absolute_box_without_width_is_shrink_to_fit`, `visibility_hidden_keeps_the_box_and_skips_the_paint`,
`opacity_zero_is_not_painted`, `enter_submits_the_enclosing_form`,
`clicking_a_submit_control_submits_the_form`, `a_js_handler_can_cancel_the_submission`,
`google_search_field_accepts_text`, mais 6 em `svg.rs`.

`naro --compat tests/compat` → **25 pass, 0 falhas, 1 unsup (26)**, inalterado.

Regressão nas páginas de referência (nenhum texto perdido, checagem por substring
em todos os runs):

| Página | Etapa 5 | Agora | JS ERROR |
|---|---|---|---|
| `naro://start` | 606,40 | 606,40 | 0 = 0 |
| `example.com` | 776,60 | 820,60 | 0 = 0 |
| `rust-lang.org` | 3940,18 | 3911,38 | 0 = 0 |
| MDN | 16378,00 | 16204,00 | 6 = 6 |

`example.com` cresceu porque seu `body{padding:25vh 2em 2em}` agora convive com um
`<style>` injetado por `insertAdjacentHTML` que antes não era pintado (SVG). As
demais variações são de layout, não de conteúdo.

`--bench`: style 0,22–0,24 ms, layout 0,059–0,075 ms — **mais rápido** que na
Etapa 5 (0,43/0,084), porque `parse_declarations` deixou de copiar o nome duas
vezes por declaração.

## 7. Limitações restantes

- **`JS ERROR` do Google: 8** (antes do UA: 0, porque os scripts não baixavam).
  Todos são APIs ausentes, não defeitos de layout: `XMLHttpRequest`, `fetch`,
  `Element.prototype.append` em coleções, `document.currentScript`, `google.cdo.load`.
  Implementar XHR/fetch é escopo de rede, não desta correção.
- **SVG**: sem `stroke`, `transform`, `gradient`, `clip-path`, `<use>` ou arcos
  elípticos reais (viraram retas). Suficiente para ícones e wordmarks.
- **`text-align` com átomos**: a linha inteira é deslocada; um átomo mais largo que
  a linha não é recortado.
- **`justify-self`** é parseado mas aplicado por item apenas no grid; em flex não
  existe.
- **Submit `POST`** cai para `GET` (não há corpo de requisição).
- **`box-shadow`** continua sem pintura (parseado, não desenhado) — a sombra da
  barra de pesquisa não aparece.
- **`opacity`** é multiplicada na herança, não composta em camada: um filho opaco
  dentro de um pai translúcido não recupera opacidade.
- **Bordas arredondadas** são preenchidas sem recortar o conteúdo.
- Nada de `float`, `position: sticky`, `border-collapse`.

## 8. Etapas anteriores

1 (seletores/cascata), 2 (unidades/funções), 3 (invalidação), 4 (DOM/medição) e
5 (layout) preservadas: `flush`/`Invalidation` intactos, `ElementBox` intacto,
`Dim`/`LengthCtx` estendidos só com `parent_opacity`. `geometry_differs` ganhou
`justify_items`, então mudar `justify-items` provoca relayout.
