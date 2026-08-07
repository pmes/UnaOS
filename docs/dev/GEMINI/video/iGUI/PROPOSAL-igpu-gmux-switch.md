# Proposal: iGPU gmux Switch (Path 1)

I propose executing the gmux switch to route the panel to the iGPU. This leverages our prior gmux groundwork to fully arm the blitter machinery we just landed, validating the entire acceleration path end-to-end on hardware immediately. Crucially, eliminating the 397 ms Kepler bring-up provides a massive, structural win for the overall boot baseline, making it the most impactful next step for the GUI milestone.
