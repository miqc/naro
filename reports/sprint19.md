# Sprint 19 — Posicionamento CSS e campo de pesquisa

Captura final: `reports/google_s19.png` (comparar com `google_s18.png`).

## Causas confirmadas

| # | Sintoma | Evidência | Causa |
|---|---|---|---|
| 1 | Campo de pesquisa invisível | `input rect=(271,199.2,458,32) bg=#ffffff border=1` **e** `clip=(0,0,1000,0)` | O `clip` herdado de um ancestral `overflow:hidden` era calculado **antes** da altura do bloco ser conhecida, com `h=0`. Todo o conteúdo abaixo da primeira caixa era recortado |
| 2 | Ícone do teclado solto em x=458 | `<img style="position:absolute;right:5px;top:4px">` | Imagens não liam `position` do atributo `style`; participavam do fluxo inline |
| 3 | Campo sem aparência | nenhuma regra com `border-radius` chegava ao layout | `border-radius` e `box-shadow` não existiam no parser |

## Implementação

| Frente | Arquivo | Detalhe |
|---|---|---|
| **Clip corrigido** | `naro-layout` | `overflow` grava um clip com `h = -1` (sentinela) e o valor é **corrigido depois** que a altura é calculada, para todos os descendentes daquele bloco |
| **Imagens absolutas** | `naro-layout` | `ImageNode` lê `position`, `top/right/bottom/left` do `style` inline; sai do fluxo inline e é posicionada contra o ancestral posicionado |
| **Aparência de controles** | `naro-css`, `naro-paint`, `naro-engine` | `border-radius`, `box-shadow` (x, y, blur, cor); `fill_round_rect` no rasterizador; sombra aproximada por 3 retângulos translúcidos; UA de input/button ajustada para `padding:1px 2px; border:1px solid #767676` |
| **APIs JS** | `naro-js` | `requestIdleCallback`/`cancelIdleCallback` (viram timers sob o mesmo orçamento), `Image` (objeto que registra `src`, sem carregar), `PerformanceObserver` vazio, `performance.now`, `requestAnimationFrame` |

## Testes

`cargo test --workspace`: **60 passando, 0 falhas** (2 novos):

- `clip_height_is_patched_after_layout` — clip herdado tem altura real (>20 px), não zero.
- `absolute_image_leaves_the_inline_flow` — texto após uma imagem absoluta fica na mesma posição de quando ela não existe.

`naro --compat tests/compat`: **17 PASS, 0 FAIL, 1 UNSUP (total 18)**.
Novo caso `18-controls.html` (input com raio/sombra, placeholder, ícone absoluto).

## Capturas antes/depois

| Aspecto | `google_s18.png` | `google_s19.png` |
|---|---|---|
| Campo de pesquisa | ausente (recortado) | **visível, 458×32, borda preta, centralizado em x=271** |
| Ícone do teclado | flutuando em x=458 | **fora do campo, em x≈968** (o wrapper não é `position:relative` no HTML real) |
| Logo | centralizado | centralizado |
| Botões | centralizados | centralizados, agora com fundo e borda |
| Rodapé | abaixo do conteúdo | abaixo do conteúdo |

## Erros JavaScript restantes

```
<script> #2: not a function @ at <anonymous> (eval_script:78:28)
<script> #8: not a function @ at <anonymous> (eval_script:11:28)
<script> #9: not a function @ at <anonymous> (eval_script:40:28)
```

Os stubs de `Image`/`requestIdleCallback`/`PerformanceObserver` não eliminaram os três `TypeError`; a origem exata não foi isolada dentro do tempo. Permanecem como próximo bloqueio.

## O que continua errado

- **Ícone do teclado fora do campo**: o wrapper do Google usa `position:relative` via classe CSS aplicada por JS; como o script aborta, o wrapper não é posicionado e a imagem absoluta resolve contra o bloco contentor errado.
- **Campo sem `border-radius`/sombra**: as regras do Google vêm de `@media` e de classes aplicadas por JS; sem elas, o UA padrão (retângulo com borda) é o que aparece.
- Faixa superior sem fundo (`position: sticky` ausente), `display: contents` e Shadow DOM continuam fora de escopo.

## Disco

12 GB livres antes e depois. Três builds de release; nenhum diretório removido.
