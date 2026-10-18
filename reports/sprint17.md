# Sprint 17 — Fluxo inline compartilhado

Capturas: `reports/inline_depois.png`, `reports/forms_s17.png`, `reports/google_s17.png`.

## Causa encontrada

Havia **duas** causas, ambas no `LineBuilder`:

1. **Controles inline eram forçados a uma linha própria.** O ramo
   `FlowItem::InlineBox` posicionava a caixa, chamava `advance` e em seguida
   `flush() + reset_x()` — resíduo da sprint anterior, quando todo controle era
   `display: block`. Nenhuma verificação de largura era feita.
2. **Palavras não guardavam sua posição horizontal.** `flush` recalculava `x`
   somando `space_before + width` de todas as palavras. Qualquer átomo inline
   (imagem, input, botão) colocado entre palavras era ignorado no cálculo, e o
   texto era pintado **por cima** do controle.

O segundo problema era o mais grave: mesmo quando a quebra de linha funcionava,
as posições saíam erradas.

## Implementação

| Mudança | Detalhe |
|---|---|
| `Word.x` | offset horizontal absoluto na linha, fixado no momento da inserção |
| `LineBuilder::place_atom` / `fits` / `at_line_start` / `atom_space` | o cursor avança sobre átomos; espaço de separação é medido uma vez |
| `FlowItem::InlineBox` | calcula largura shrink-to-fit, testa se cabe, quebra se necessário, posiciona e avança — **sem flush forçado** |
| `FlowItem::Image` | mesmo caminho: cabe → mesma linha; não cabe → quebra; largura limitada à linha com proporção preservada |
| `flush` | total da linha = `max(word.x + width, line_width)`; alinhamento aplicado sobre esse total |
| Coalescência de runs | só funde palavras **adjacentes** (`w.x - last_end - space ≈ 0`), para não engolir átomos |
| Altura de linha | `atom_height` entra no cálculo do line box |
| Largura de controle | nunca menor que o próprio rótulo (evitava botões de 8 px) |

## Testes automatizados

`cargo test --workspace`: **58 passando, 0 falhas** (4 novos em `naro-layout`):

| Teste | Verifica |
|---|---|
| `inputs_and_button_share_one_line` | dois inputs e um botão na mesma linha, x crescente |
| `inline_controls_wrap_in_a_narrow_container` | 3 × 200 px em 450 px → dois na primeira linha, terceiro quebra e volta a x=0 |
| `text_and_image_share_a_line` | imagem reserva 40 px; o texto seguinte fica na mesma baseline |
| `inline_spaces_separate_links` | dois links com espaço entre eles, sem colar |

`naro --compat tests/compat`: **15 PASS, 0 FAIL, 1 UNSUP (total 16)**.
O caso novo `16-inline.html` foi adicionado e passa; o baseline anterior (14/0/1) subiu para 15/0/1.

## Validação visual

`tests/compat/16-inline.html` renderizado:

- `[um][dois][Enviar]` lado a lado na mesma linha, com bordas e fundo do botão.
- `Texto [meio] depois` — controle **entre** palavras, texto antes e depois na mesma linha.
- Contêiner de 200 px com três inputs de 120 px: um por linha, sem sobreposição.

`14-form.html` continua correto (rótulos, campos e botão alinhados à esquerda).

## Google

| Aspecto | Sprint 16 | Agora |
|---|---|---|
| Logo | centralizado | centralizado (igual) |
| Campos de busca | 458×32, um por linha | **inline com o botão**, na mesma linha |
| Botões "Pesquisa Google" / "Estou com sorte" | texto solto | texto com largura própria, sem colisão |
| Sobreposição de texto | havia | **não há** |

O campo de busca continua visualmente vazio: o Google preenche `aria-label`/placeholder por JS, e o script aborta em `self is not defined`. Isso é limitação de runtime, não de layout.

## Limitações restantes

- Alinhamento vertical é por linha de base aproximada: um átomo mais alto que o texto empurra a linha, mas não há `vertical-align`.
- Dois controles inline consecutivos ficam colados (sem espaço no HTML) — correto, mas sem `margin` padrão de UA.
- `display: contents`, Shadow DOM, `position: fixed` continuam ausentes e afetam o restante do Google.
- Sem `white-space: nowrap` nem `text-overflow`.

## Disco

12 GB livres antes e depois. Três builds de release; nenhum diretório removido.
